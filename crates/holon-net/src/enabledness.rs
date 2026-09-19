//! Whether each transition of the compiled net may fire for one subject.
//!
//! A pure function of the net and a [`Marking`]: no store, no async, no state.
//!
//! Guard predicates are not evaluated here. An arc's `refinement` and a
//! transition's `residue` belong to the guard language, which
//! `docs/adr/0032-petri-net-execution-semantics.md` deferred item 6 keeps
//! separate from the arc language; both yield [`Offer::Unknown`].

use holon_api::EntityUri;
use holon_pattern::arcs::ArcRelation;

use crate::bridge::TransitionKey;
use crate::bridge::TransitionSource;
use crate::marking::Marking;
use crate::net::Analyzability;
use crate::net::CompiledNet;
use crate::net::Flow;
use crate::net::NetTransition;

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

/// One arc's contribution to its transition's offer.
enum ArcVerdict {
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

    // Enabledness rests on what a firing reads, consumes or relocates; a
    // produced place says nothing about whether the firing may happen.
    let verdicts = transition
        .arcs
        .iter()
        .filter(|arc| matches!(arc.flow, Flow::Read | Flow::Consume | Flow::Relocate))
        .map(|arc| {
            if arc.refinement.is_some() {
                return ArcVerdict::CannotSay(format!(
                    "the arc on `{}` carries a guard refinement",
                    arc.place
                ));
            }
            if &arc.place.relation != subject_relation {
                return ArcVerdict::CannotSay(format!(
                    "`{}` lies outside a marking scoped to one `{subject_relation}`",
                    arc.place
                ));
            }
            // An operation targets an existing subject — Model.md invariant
            // 15 (D147.a; landed with the set_field changed-row assert).
            // `MoveGuard` instead confirms a subject its reader cannot
            // classify, so for the operations the net can analyze this
            // withholds more than the dispatcher refuses.
            if !marking.present(subject_relation, subject) {
                return ArcVerdict::Refuses(format!(
                    "`{subject}` does not exist, and the operation reads `{}`",
                    arc.place
                ));
            }
            ArcVerdict::Satisfied
        });

    // Every arc is weighed. A refusal anywhere outranks an unclassifiable arc
    // anywhere, so the verdict does not depend on declaration order.
    let mut cannot_say = None;
    for verdict in verdicts {
        match verdict {
            ArcVerdict::Refuses(reason) => return Offer::Refused { reason },
            ArcVerdict::CannotSay(why) if cannot_say.is_none() => cannot_say = Some(why),
            ArcVerdict::CannotSay(_) | ArcVerdict::Satisfied => {}
        }
    }
    match cannot_say {
        Some(why) => Offer::Unknown { why },
        None => Offer::Enabled,
    }
}
