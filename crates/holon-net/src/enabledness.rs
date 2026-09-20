//! Whether each transition of the compiled net may fire for one subject.
//!
//! A pure function of the net and a [`Marking`]: no store, no async, no state.
//!
//! A transition offers per FIRING MODE, and the modes combine as a Kleene
//! disjunction: one enabled mode enables the transition, and only a
//! transition every mode refuses is refused. A correlated group inside a mode
//! binds ONE token, so all of its arcs speak about the same entity.
//!
//! A transition's `residue` still belongs to the guard language, which
//! `docs/adr/0032-petri-net-execution-semantics.md` deferred item 6 keeps
//! separate from the arc language, and yields [`Offer::Unknown`].

use holon_api::EntityUri;
use holon_pattern::Value;
use holon_pattern::arcs::ArcPlace;
use holon_pattern::arcs::ArcRelation;
use holon_pattern::pattern::compare_2valued;

use crate::bridge::TransitionKey;
use crate::bridge::TransitionSource;
use crate::marking::Marking;
use crate::net::Analyzability;
use crate::net::CompiledNet;
use crate::net::CorrelatedGroup;
use crate::net::Flow;
use crate::net::HopDirection;
use crate::net::NetArc;
use crate::net::NetTransition;
use crate::net::Refinement;
use crate::net::TransitionMode;

/// What the offer path says about one operation for one subject.
///
/// Three-valued where `NetGuard`'s verdict is binary: the dispatcher remains
/// the authority, so a transition the marking cannot classify is offered and
/// decided on dispatch. Only [`Offer::Refused`] withholds an operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offer {
    Enabled,
    Refused { reason: String },
    Unknown { why: String },
}

impl Offer {
    /// Whether a surface should show this operation.
    pub fn is_offered(&self) -> bool {
        !matches!(self, Offer::Refused { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpEnabledness {
    pub key: TransitionKey,
    pub offer: Offer,
}

/// Every transition's offer for `subject`, in the net's own order.
///
/// `subject_relation` is the relation `subject` belongs to; the marking is
/// scoped to it, so a transition reaching outside it cannot be decided here.
pub fn evaluate(
    net: &CompiledNet,
    marking: &dyn Marking,
    subject_relation: &ArcRelation,
    subject: &EntityUri,
) -> Vec<OpEnabledness> {
    net.transitions
        .iter()
        .map(|transition| OpEnabledness {
            key: transition.key(),
            offer: offer_for(transition, marking, subject_relation, subject),
        })
        .collect()
}

/// One arc's, one group's, or one mode's contribution to the verdict.
enum Verdict {
    Satisfied,
    Refuses(String),
    CannotSay(String),
}

fn offer_for(
    transition: &NetTransition,
    marking: &dyn Marking,
    subject_relation: &ArcRelation,
    subject: &EntityUri,
) -> Offer {
    if let Analyzability::Unanalyzable { undeclared } = &transition.analyzability {
        let halves = undeclared
            .iter()
            .map(|half| format!("{half:?}"))
            .collect::<Vec<_>>()
            .join(" and no ");
        return Offer::Unknown {
            why: format!("the transition declares no {halves}"),
        };
    }

    match &transition.source {
        TransitionSource::Operation { entity, .. }
            if entity.as_str() != subject_relation.as_str() =>
        {
            return Offer::Unknown {
                why: format!(
                    "the operation acts on `{entity}`, not on the subject's `{subject_relation}`"
                ),
            };
        }
        TransitionSource::Rule { name, .. } => {
            return Offer::Unknown {
                why: format!("`{name}` is a rule, fired by the net rather than requested"),
            };
        }
        TransitionSource::Operation { .. } => {}
    }

    if !transition.residue.is_empty() {
        return Offer::Unknown {
            why: format!(
                "{} guard predicate(s) outside the arc language",
                transition.residue.len()
            ),
        };
    }

    // Kleene disjunction over the modes: one enabled mode is enough, and only
    // a transition every mode refuses is refused.
    let mut refusals = Vec::new();
    let mut cannot_say = None;
    for mode in &transition.modes {
        match mode_verdict(mode, marking, subject_relation, subject) {
            Verdict::Satisfied => return Offer::Enabled,
            Verdict::Refuses(reason) => refusals.push(reason),
            Verdict::CannotSay(why) if cannot_say.is_none() => cannot_say = Some(why),
            Verdict::CannotSay(_) => {}
        }
    }
    match cannot_say {
        Some(why) => Offer::Unknown { why },
        None => Offer::Refused {
            reason: refusals.join("; and "),
        },
    }
}

fn mode_verdict(
    mode: &TransitionMode,
    marking: &dyn Marking,
    subject_relation: &ArcRelation,
    subject: &EntityUri,
) -> Verdict {
    let mut cannot_say = None;
    // Enabledness rests on what a firing reads, consumes or relocates; a
    // produced place says nothing about whether the firing may happen.
    for arc in mode
        .arcs
        .iter()
        .filter(|a| matches!(a.flow, Flow::Read | Flow::Consume | Flow::Relocate))
    {
        match subject_arc_verdict(arc, marking, subject_relation, subject) {
            Verdict::Refuses(reason) => return Verdict::Refuses(reason),
            Verdict::CannotSay(why) if cannot_say.is_none() => cannot_say = Some(why),
            Verdict::CannotSay(_) | Verdict::Satisfied => {}
        }
    }
    for group in &mode.hops {
        match group_verdict(group, marking, subject_relation, subject) {
            Verdict::Refuses(reason) => return Verdict::Refuses(reason),
            Verdict::CannotSay(why) if cannot_say.is_none() => cannot_say = Some(why),
            Verdict::CannotSay(_) | Verdict::Satisfied => {}
        }
    }
    match cannot_say {
        Some(why) => Verdict::CannotSay(why),
        None => Verdict::Satisfied,
    }
}

fn subject_arc_verdict(
    arc: &NetArc,
    marking: &dyn Marking,
    subject_relation: &ArcRelation,
    subject: &EntityUri,
) -> Verdict {
    if &arc.place.relation != subject_relation {
        return Verdict::CannotSay(format!(
            "`{}` lies outside a marking scoped to one `{subject_relation}`",
            arc.place
        ));
    }
    // An operation targets an existing subject — Model.md invariant 15
    // (D147.a). `MoveGuard` instead confirms a subject its reader cannot
    // classify, so for the operations the net can analyze this withholds more
    // than the dispatcher refuses.
    if !marking.present(subject_relation, subject) {
        return Verdict::Refuses(format!(
            "`{subject}` does not exist, and the operation reads `{}`",
            arc.place
        ));
    }
    match &arc.refinement {
        None => Verdict::Satisfied,
        Some(refinement) => match refinement_holds(refinement, marking, arc, subject) {
            Some(true) => Verdict::Satisfied,
            Some(false) => Verdict::Refuses(format!(
                "`{subject}` does not satisfy the guard on `{}`",
                arc.place
            )),
            None => Verdict::CannotSay(format!(
                "the refinement on `{}` is not decidable from a marking",
                arc.place
            )),
        },
    }
}

/// A hop is satisfied when SOME reached token satisfies EVERY arc of the
/// group. The single binding is what makes this a join rather than a
/// conjunction of independent existentials.
fn group_verdict(
    group: &CorrelatedGroup,
    marking: &dyn Marking,
    subject_relation: &ArcRelation,
    subject: &EntityUri,
) -> Verdict {
    for hop in &group.correlation.hops {
        for place in [&hop.from, &hop.to] {
            if &place.relation != subject_relation {
                return Verdict::CannotSay(format!(
                    "the hop through `{place}` leaves a marking scoped to one \
                     `{subject_relation}`"
                ));
            }
        }
    }
    if !marking.present(subject_relation, subject) {
        return Verdict::Refuses(format!(
            "`{subject}` does not exist, so the hop `${}` binds nothing",
            group.binding.as_str()
        ));
    }

    let mut reached = vec![subject.clone()];
    for hop in &group.correlation.hops {
        let (read, lookup) = match hop.direction {
            HopDirection::Forward => (&hop.from, &hop.to),
            HopDirection::Inverse => (&hop.to, &hop.from),
        };
        let mut next: Vec<EntityUri> = Vec::new();
        for current in &reached {
            let Some(value) = cell(marking, read, current) else {
                continue;
            };
            for entity in marking.matching(lookup, &value) {
                if !next.contains(&entity) {
                    next.push(entity);
                }
            }
        }
        reached = next;
    }
    if group.correlation.exclude_subject {
        reached.retain(|entity| entity != subject);
    }

    let mut undecidable = None;
    for token in &reached {
        let mut all = true;
        for arc in &group.arcs {
            let refinement = arc
                .refinement
                .as_ref()
                .expect("a correlated arc carries the refinement it was compiled from");
            match refinement_holds(refinement, marking, arc, token) {
                Some(true) => {}
                Some(false) => {
                    all = false;
                    break;
                }
                None => {
                    undecidable = Some(format!(
                        "the refinement on `{}` is not decidable from a marking",
                        arc.place
                    ));
                    all = false;
                    break;
                }
            }
        }
        if all {
            return Verdict::Satisfied;
        }
    }
    match undecidable {
        Some(why) => Verdict::CannotSay(why),
        None => Verdict::Refuses(format!(
            "no entity reached by `${}` satisfies the guard",
            group.binding.as_str()
        )),
    }
}

/// Every cell the net reads, with the [`Marking::value`] contract enforced
/// here rather than trusted at each implementor.
///
/// A marking that reported an empty cell as `Some(Value::Null)` would decide
/// verdicts by itself: `Null` compares equal to `Null`, so a hop would
/// correlate every row with an empty cell into one family and answer `Enabled`
/// where `holon_pattern`'s two legs answer `false`. The implementors are all
/// in-tree, so this is a programming error and asserts rather than degrading
/// to [`Offer::Unknown`] — an undecidable verdict would hide the broken
/// marking instead of naming it.
fn cell(marking: &dyn Marking, place: &ArcPlace, entity: &EntityUri) -> Option<Value> {
    let read = marking.value(place, entity);
    assert!(
        read != Some(Value::Null),
        "the marking reported `{place}` of `{entity}` as Value::Null; the contract is that an \
         empty cell reads `None`"
    );
    read
}

/// `None` when the marking cannot decide the shape — today only tag
/// membership, which is a list rather than a cell and needs its own question.
///
/// Every other shape is decided by `holon_pattern`'s own comparison, so an arc
/// and the guard conjunct it was compiled from cannot answer differently.
fn refinement_holds(
    refinement: &Refinement,
    marking: &dyn Marking,
    arc: &NetArc,
    entity: &EntityUri,
) -> Option<bool> {
    match refinement {
        Refinement::Cell { op, rhs } => {
            let read = cell(marking, &arc.place, entity);
            Some(compare_2valued(read.as_ref(), *op, rhs))
        }
        Refinement::Keyed { key, op, rhs } => match cell(marking, &arc.place, entity) {
            // An absent bag is an absent key, which a 2-valued comparison
            // never matches. A bag that is NOT a map is a shape error at the
            // marking boundary, and answering it `false` would be a confident
            // refusal built on malformed data.
            None => Some(compare_2valued(None, *op, rhs)),
            Some(bag) => bag
                .as_object()
                .map(|o| compare_2valued(o.get(key), *op, rhs)),
        },
        Refinement::HasTag(_) => None,
    }
}
