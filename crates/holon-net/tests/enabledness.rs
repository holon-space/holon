//! Offer verdicts of `holon_net::enabledness` for one subject.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use holon_api::EntityUri;
use holon_api::Value;
use holon_net::Analyzability;
use holon_net::ArcOrigin;
use holon_net::BindingVar;
use holon_net::CompiledNet;
use holon_net::CorrelatedGroup;
use holon_net::Correlation;
use holon_net::Flow;
use holon_net::GuardResidue;
use holon_net::Hop;
use holon_net::NetArc;
use holon_net::NetEntity;
use holon_net::NetTransition;
use holon_net::Refinement;
use holon_net::TransitionMode;
use holon_net::TransitionSource;
use holon_net::UndeclaredHalf;
use holon_net::enabledness::Offer;
use holon_net::enabledness::evaluate;
use holon_net::marking::Marking;
use holon_pattern::arcs::ArcPlace;
use holon_pattern::arcs::ArcRelation;
use holon_pattern::pattern::CmpOp;
use holon_pattern::pattern::Pattern;

/// A marking holding present entities and, for the hop and refinement cases,
/// their cells.
struct Rows {
    present: BTreeSet<String>,
    cells: BTreeMap<(String, String), Value>,
}

impl Rows {
    fn holding(ids: &[&str]) -> Self {
        Rows {
            present: ids.iter().map(|s| (*s).to_string()).collect(),
            cells: BTreeMap::new(),
        }
    }
    fn empty() -> Self {
        Rows {
            present: BTreeSet::new(),
            cells: BTreeMap::new(),
        }
    }
    fn with_cell(mut self, entity: &str, place: &str, value: &str) -> Self {
        self.present.insert(entity.to_string());
        self.cells.insert(
            (entity.to_string(), place.to_string()),
            Value::String(value.to_string()),
        );
        self
    }

    /// A marking that breaks the [`Marking::value`] contract by reporting an
    /// empty cell as `Some(Value::Null)` instead of `None`.
    fn with_null_cell(mut self, entity: &str, place: &str) -> Self {
        self.present.insert(entity.to_string());
        self.cells
            .insert((entity.to_string(), place.to_string()), Value::Null);
        self
    }
}

impl Marking for Rows {
    fn present(&self, _: &ArcRelation, entity: &EntityUri) -> bool {
        self.present.contains(&entity.to_string())
    }

    fn value(&self, place: &ArcPlace, entity: &EntityUri) -> Option<Value> {
        self.cells
            .get(&(entity.to_string(), place.to_string()))
            .cloned()
    }

    fn matching(&self, to: &ArcPlace, value: &Value) -> Vec<EntityUri> {
        self.cells
            .iter()
            .filter(|((_, place), cell)| place == &to.to_string() && *cell == value)
            .map(|((entity, _), _)| EntityUri::parse(entity).expect("a seeded uri"))
            .collect()
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
        modes: vec![TransitionMode::new(arcs)],
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
        cause: holon_net::guards::ResidueCause::UnrefinableHop,
    }];
    let offer = sole_offer(&net_of(vec![t]), &Rows::holding(&["block:subject"]));
    assert!(
        matches!(&offer, Offer::Unknown { why } if why.contains("guard predicate")),
        "a residue predicate belongs to the gate, not here: {offer:?}"
    );
}

fn refined(field: &str, expected: &str) -> NetArc {
    let mut a = arc(field, Flow::Read);
    a.origin = ArcOrigin::GuardRefinement;
    a.refinement = Some(Refinement::Cell {
        op: CmpOp::Eq,
        rhs: Value::String(expected.to_string()),
    });
    a
}

#[test]
fn a_cell_refinement_that_holds_enables() {
    let offer = sole_offer(
        &net_of(vec![transition(
            "set_field",
            vec![refined("content", "hello")],
        )]),
        &Rows::empty().with_cell("block:subject", "block.content", "hello"),
    );
    assert_eq!(offer, Offer::Enabled);
}

#[test]
fn a_cell_refinement_that_fails_refuses() {
    let offer = sole_offer(
        &net_of(vec![transition(
            "set_field",
            vec![refined("content", "hello")],
        )]),
        &Rows::empty().with_cell("block:subject", "block.content", "goodbye"),
    );
    assert!(
        matches!(&offer, Offer::Refused { reason } if reason.contains("block.content")),
        "a decided refinement that fails must refuse: {offer:?}"
    );
}

/// Tag membership is a list, not a cell, so the marking cannot decide it yet.
#[test]
fn a_tag_refinement_is_unknown_not_guessed() {
    let mut a = arc("tags", Flow::Read);
    a.origin = ArcOrigin::GuardRefinement;
    a.refinement = Some(Refinement::HasTag("Page".to_string()));
    let offer = sole_offer(
        &net_of(vec![transition("set_field", vec![a])]),
        &Rows::holding(&["block:subject"]),
    );
    assert!(
        matches!(&offer, Offer::Unknown { why } if why.contains("refinement")),
        "a tag refinement must not be guessed: {offer:?}"
    );
}

/// One bound token must satisfy every arc of a hop. Two entities each
/// satisfying one half is NOT a match — the join, not two existentials.
#[test]
fn a_hop_binds_one_token_for_all_its_arcs() {
    let group = CorrelatedGroup {
        binding: BindingVar::new("hop0"),
        correlation: Correlation {
            hops: vec![Hop::parent(), Hop::child()],
            exclude_subject: false,
        },
        arcs: vec![
            refined("content_type", "source"),
            refined("source_language", "holon_rule"),
        ],
    };
    let mut t = transition("move_block", vec![arc("id", Flow::Read)]);
    t.modes[0].hops.push(group);
    let net = net_of(vec![t]);

    // Two siblings, one per half: no single token satisfies both.
    let split = Rows::empty()
        .with_cell("block:home", "block.id", "block:home")
        .with_cell("block:subject", "block.parent_id", "block:home")
        .with_cell("block:subject", "block.id", "block:subject")
        .with_cell("block:one", "block.parent_id", "block:home")
        .with_cell("block:one", "block.content_type", "source")
        .with_cell("block:two", "block.parent_id", "block:home")
        .with_cell("block:two", "block.source_language", "holon_rule");
    assert!(
        matches!(sole_offer(&net, &split), Offer::Refused { .. }),
        "two entities each satisfying one half must not enable the hop"
    );

    // One sibling carrying both cells does enable it.
    let joined = Rows::empty()
        .with_cell("block:home", "block.id", "block:home")
        .with_cell("block:subject", "block.parent_id", "block:home")
        .with_cell("block:subject", "block.id", "block:subject")
        .with_cell("block:both", "block.parent_id", "block:home")
        .with_cell("block:both", "block.content_type", "source")
        .with_cell("block:both", "block.source_language", "holon_rule");
    assert_eq!(sole_offer(&net, &joined), Offer::Enabled);
}

/// A sibling hop over two blocks whose parent cell is EXPLICITLY null.
///
/// `holon_pattern`'s in-memory and SQL legs both answer that an absent parent
/// has no siblings. The net can only answer the same if no marking may report
/// an empty cell as `Some(Value::Null)`: hopping on that value would correlate
/// every parentless block into one family. The contract makes it a programming
/// error, and the net enforces it rather than trusting each implementor.
#[test]
#[should_panic(expected = "reported `block.parent_id` of `block:subject` as Value::Null")]
fn a_marking_reporting_an_explicit_null_cell_is_a_contract_violation() {
    let group = CorrelatedGroup {
        binding: BindingVar::new("hop0"),
        correlation: Correlation {
            hops: vec![Hop::sibling()],
            exclude_subject: true,
        },
        arcs: vec![refined("content_type", "source")],
    };
    let mut t = transition("move_block", vec![arc("id", Flow::Read)]);
    t.modes[0].hops.push(group);
    let net = net_of(vec![t]);

    let two_roots = Rows::empty()
        .with_null_cell("block:subject", "block.parent_id")
        .with_cell("block:subject", "block.id", "block:subject")
        .with_null_cell("block:other-root", "block.parent_id")
        .with_cell("block:other-root", "block.id", "block:other-root")
        .with_cell("block:other-root", "block.content_type", "source");
    sole_offer(&net, &two_roots);
}

/// One enabled mode enables the transition; only a transition every mode
/// refuses is refused.
#[test]
fn modes_combine_as_a_kleene_disjunction() {
    let mut t = transition("set_field", vec![refined("content", "hello")]);
    t.modes
        .push(TransitionMode::new(vec![refined("content", "goodbye")]));
    let net = net_of(vec![t]);
    assert_eq!(
        sole_offer(
            &net,
            &Rows::empty().with_cell("block:subject", "block.content", "goodbye")
        ),
        Offer::Enabled,
        "the second mode holds, so the transition is enabled"
    );
    assert!(
        matches!(
            sole_offer(
                &net,
                &Rows::empty().with_cell("block:subject", "block.content", "other")
            ),
            Offer::Refused { .. }
        ),
        "no mode holds, so the transition is refused"
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
