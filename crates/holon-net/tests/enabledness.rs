//! Offer verdicts of `holon_net::enabledness` for one subject.

use std::collections::BTreeSet;

use holon_api::EntityUri;
use holon_api::Value;
use holon_net::Analyzability;
use holon_net::ArcOrigin;
use holon_net::CompiledNet;
use holon_net::Flow;
use holon_net::GuardResidue;
use holon_net::NetArc;
use holon_net::NetEntity;
use holon_net::NetTransition;
use holon_net::TransitionSource;
use holon_net::UndeclaredHalf;
use holon_net::enabledness::Offer;
use holon_net::enabledness::evaluate;
use holon_net::marking::Marking;
use holon_pattern::arcs::ArcPlace;
use holon_pattern::arcs::ArcRelation;
use holon_pattern::pattern::CmpOp;
use holon_pattern::pattern::FieldRef;
use holon_pattern::pattern::Operand;
use holon_pattern::pattern::Pattern;

/// A marking holding a fixed set of present entities. Enough for enabledness,
/// which asks only presence and place values.
struct Rows {
    present: BTreeSet<String>,
}

impl Rows {
    fn holding(ids: &[&str]) -> Self {
        Rows {
            present: ids.iter().map(|s| (*s).to_string()).collect(),
        }
    }
    fn empty() -> Self {
        Rows {
            present: BTreeSet::new(),
        }
    }
}

impl Marking for Rows {
    fn present(&self, _: &ArcRelation, entity: &EntityUri) -> bool {
        self.present.contains(&entity.to_string())
    }
}

fn block() -> ArcRelation {
    ArcRelation::block()
}

fn subject() -> EntityUri {
    EntityUri::parse("block:subject").expect("a block uri")
}

fn arc(field: &str, flow: Flow) -> NetArc {
    NetArc {
        place: ArcPlace::new("block", field),
        flow,
        origin: ArcOrigin::DeclaredRead,
        refinement: None,
        binding: None,
    }
}

fn transition(op: &str, arcs: Vec<NetArc>) -> NetTransition {
    NetTransition {
        source: TransitionSource::Operation {
            entity: NetEntity::parse("block").expect("dotless"),
            op: op.to_string(),
        },
        analyzability: Analyzability::Analyzable,
        arcs,
        residue: vec![],
    }
}

fn net_of(transitions: Vec<NetTransition>) -> CompiledNet {
    CompiledNet { transitions }
}

fn sole_offer(net: &CompiledNet, marking: &dyn Marking) -> Offer {
    let mut out = evaluate(net, marking, &block(), &subject());
    assert_eq!(out.len(), 1, "these cases carry one transition");
    out.remove(0).offer
}

#[test]
fn an_absent_subject_is_refused_for_a_transition_that_reads_it() {
    let net = net_of(vec![transition(
        "set_field",
        vec![arc("content", Flow::Read)],
    )]);
    let offer = sole_offer(&net, &Rows::empty());
    match &offer {
        Offer::Refused { reason } => {
            assert!(
                reason.contains("block:subject") && reason.contains("does not exist"),
                "the refusal must name the missing subject: {reason}"
            );
        }
        other => panic!("an absent subject must be refused, got {other:?}"),
    }
    assert!(!offer.is_offered(), "a refused op is not offered");
}

#[test]
fn a_present_subject_enables_a_fully_declared_transition() {
    let net = net_of(vec![transition(
        "set_field",
        vec![arc("content", Flow::Read)],
    )]);
    assert_eq!(
        sole_offer(&net, &Rows::holding(&["block:subject"])),
        Offer::Enabled
    );
}

#[test]
fn a_produce_only_arc_does_not_gate_enabledness() {
    let net = net_of(vec![transition("create", vec![arc("id", Flow::Produce)])]);
    assert_eq!(sole_offer(&net, &Rows::empty()), Offer::Enabled);
}

#[test]
fn an_unanalyzable_transition_is_unknown_and_offered() {
    let mut t = transition("delete", vec![]);
    t.analyzability = Analyzability::Unanalyzable {
        undeclared: vec![UndeclaredHalf::Arcs, UndeclaredHalf::MarkingDelta],
    };
    let offer = sole_offer(&net_of(vec![t]), &Rows::empty());
    match &offer {
        Offer::Unknown { why } => assert!(
            why.contains("Arcs") && why.contains("MarkingDelta"),
            "the reason must name BOTH undeclared halves: {why}"
        ),
        other => panic!("an unanalyzable transition must be Unknown, got {other:?}"),
    }
    assert!(offer.is_offered(), "Unknown is offered — dispatch decides");
}

#[test]
fn a_transition_with_guard_residue_is_unknown() {
    let mut t = transition("move_block", vec![arc("parent_id", Flow::Relocate)]);
    t.residue = vec![GuardResidue {
        predicate: Pattern::Parent(Box::new(Pattern::HasTag("Page".into()))),
    }];
    let offer = sole_offer(&net_of(vec![t]), &Rows::holding(&["block:subject"]));
    assert!(
        matches!(&offer, Offer::Unknown { why } if why.contains("guard predicate")),
        "a residue predicate belongs to the gate, not here: {offer:?}"
    );
}

#[test]
fn a_guard_refinement_is_unknown_not_guessed() {
    let mut a = arc("content", Flow::Read);
    a.origin = ArcOrigin::GuardRefinement;
    a.refinement = Some(Pattern::Field {
        field: FieldRef::Column {
            relation: "block".into(),
            name: "content".into(),
        },
        op: CmpOp::Eq,
        rhs: Operand::Lit(Value::String("hello".into())),
    });
    let offer = sole_offer(
        &net_of(vec![transition("set_field", vec![a])]),
        &Rows::holding(&["block:subject"]),
    );
    assert!(
        matches!(&offer, Offer::Unknown { why } if why.contains("refinement")),
        "a refined arc must not be guessed: {offer:?}"
    );
}

#[test]
fn a_read_of_another_relation_is_unknown() {
    let net = net_of(vec![transition(
        "due_today",
        vec![NetArc {
            place: ArcPlace::new("clock", "today"),
            flow: Flow::Read,
            origin: ArcOrigin::DeclaredRead,
            refinement: None,
            binding: None,
        }],
    )]);
    let offer = sole_offer(&net, &Rows::holding(&["block:subject"]));
    assert!(
        matches!(&offer, Offer::Unknown { why } if why.contains("clock.today")),
        "an out-of-scope relation must say so: {offer:?}"
    );
}

#[test]
fn every_transition_gets_exactly_one_offer_in_net_order() {
    let net = net_of(vec![
        transition("a", vec![arc("content", Flow::Read)]),
        transition("b", vec![arc("id", Flow::Produce)]),
    ]);
    let out = evaluate(
        &net,
        &Rows::holding(&["block:subject"]),
        &block(),
        &subject(),
    );
    let keys: Vec<String> = out.iter().map(|o| o.key.as_str().to_string()).collect();
    assert_eq!(keys, vec!["op:block.a", "op:block.b"], "net order is kept");
}

#[test]
fn a_definite_refusal_outranks_an_unclassifiable_arc_in_either_order() {
    // One arc refuses (own relation, absent subject), one cannot be judged
    // (foreign relation). The verdict must not depend on which comes first.
    let own = arc("content", Flow::Read);
    let foreign = NetArc {
        place: ArcPlace::new("clock", "today"),
        flow: Flow::Read,
        origin: ArcOrigin::DeclaredRead,
        refinement: None,
        binding: None,
    };
    let own_first = sole_offer(
        &net_of(vec![transition("t", vec![own.clone(), foreign.clone()])]),
        &Rows::empty(),
    );
    let foreign_first = sole_offer(
        &net_of(vec![transition("t", vec![foreign, own])]),
        &Rows::empty(),
    );
    assert_eq!(
        own_first, foreign_first,
        "arc declaration order changed the verdict: {own_first:?} vs {foreign_first:?}"
    );
    assert!(
        matches!(own_first, Offer::Refused { .. }),
        "a definite refusal outranks an unclassifiable arc: {own_first:?}"
    );
}

#[test]
fn an_operation_on_another_entity_is_unknown_for_this_subject() {
    let mut t = transition("declare_type", vec![]);
    t.source = TransitionSource::Operation {
        entity: NetEntity::parse("type").expect("dotless"),
        op: "declare_type".to_string(),
    };
    let offer = sole_offer(&net_of(vec![t]), &Rows::holding(&["block:subject"]));
    assert!(
        matches!(&offer, Offer::Unknown { why } if why.contains("`type`")),
        "an op on another entity must not be judged against a block subject: {offer:?}"
    );
}

#[test]
fn a_rule_transition_is_not_offered_as_an_operation() {
    let mut t = transition("ignored", vec![]);
    t.source = TransitionSource::Rule {
        block_id: "block:rule".to_string(),
        name: "journals".to_string(),
        active: true,
    };
    let offer = sole_offer(&net_of(vec![t]), &Rows::holding(&["block:subject"]));
    assert!(
        matches!(&offer, Offer::Unknown { why } if why.contains("rule")),
        "a rule fires itself: {offer:?}"
    );
}
