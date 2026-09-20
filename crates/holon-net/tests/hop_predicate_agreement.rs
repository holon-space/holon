//! Hop semantics: the compiled net decides a composed `parent(child(...))`
//! reach the way the shape itself says it should.
//!
//! This is a FAST, storage-free lock on the hop machinery, and its reference
//! is stated in this file. It is deliberately NOT the production oracle for
//! rule machinery: that one is
//! `crates/holon-app/tests/is_program_net_agrees_with_the_profile.rs`, which
//! anchors on the profile resolver's own `is_program` computed field and on
//! `sibling(...)`. The divergence between the two reaches is pinned at the
//! bottom of this file, and it is why `sibling(...)` exists.

use holon_api::EntityUri;
use holon_net::Analyzability;
use holon_net::CompiledNet;
use holon_net::NetEntity;
use holon_net::NetTransition;
use holon_net::TransitionSource;
use holon_net::enabledness::Offer;
use holon_net::enabledness::evaluate;
use holon_net::guards::classify_guard;
use holon_net::marking::Marking;
use holon_pattern::Value;
use holon_pattern::arcs::ArcPlace;
use holon_pattern::arcs::ArcRelation;
use holon_pattern::pattern::CmpOp;
use holon_pattern::pattern::FieldRef;
use holon_pattern::pattern::Guard;
use holon_pattern::pattern::Operand;
use holon_pattern::pattern::Pattern;
use holon_pattern::pattern::Subject;
use proptest::prelude::*;

/// The sentinel a root block carries; the store keeps an explicit root parent
/// rather than a null one.
const NO_PARENT: &str = "block:no-parent";

/// The rule-machinery SHAPE reached through `parent(child(...))`.
///
/// Factored: both branches require `is_source`, so it lifts out of the
/// disjunction. The inner disjunction over the two rule languages lifts OUT
/// of the hop, because an existential distributes over a disjunction — four
/// firing modes in all.
fn col(name: &str, value: &str) -> Pattern {
    Pattern::Field {
        field: FieldRef::Column {
            relation: "block".to_string(),
            name: name.to_string(),
        },
        op: CmpOp::Eq,
        rhs: Operand::Lit(Value::String(value.to_string())),
    }
}

fn is_rule_head_pattern() -> Pattern {
    Pattern::And(vec![
        col("content_type", "source"),
        Pattern::Or(vec![
            col("source_language", "holon_rule"),
            col("source_language", "action"),
        ]),
    ])
}

fn is_program_guard() -> Guard {
    Guard {
        subject: Subject::Block,
        body: Pattern::And(vec![
            col("content_type", "source"),
            Pattern::Or(vec![
                col("source_language", "holon_rule"),
                col("source_language", "action"),
                Pattern::Parent(Box::new(Pattern::Child(Box::new(is_rule_head_pattern())))),
            ]),
        ]),
    }
}

// ------------------------------------------------------------- the population

#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    id: String,
    parent_id: String,
    content_type: String,
    source_language: String,
}

#[derive(Debug, Clone)]
struct Population {
    rows: Vec<Row>,
}

impl Population {
    fn get(&self, id: &str) -> Option<&Row> {
        self.rows.iter().find(|r| r.id == id)
    }

    fn cell(&self, id: &str, place: &str) -> Option<&str> {
        self.get(id).map(|r| match place {
            "block.id" => r.id.as_str(),
            "block.parent_id" => r.parent_id.as_str(),
            "block.content_type" => r.content_type.as_str(),
            "block.source_language" => r.source_language.as_str(),
            other => panic!("the guard named a place the fixture has no column for: {other}"),
        })
    }
}

impl Marking for Population {
    fn present(&self, _: &ArcRelation, entity: &EntityUri) -> bool {
        self.get(entity.as_str()).is_some()
    }

    fn value(&self, place: &ArcPlace, entity: &EntityUri) -> Option<Value> {
        self.cell(entity.as_str(), &place.to_string())
            .map(|v| Value::String(v.to_string()))
    }

    fn matching(&self, to: &ArcPlace, value: &Value) -> Vec<EntityUri> {
        let place = to.to_string();
        let wanted = value.as_string().expect("the fixture's cells are strings");
        self.rows
            .iter()
            .filter(|r| self.cell(&r.id, &place) == Some(wanted))
            .map(|r| EntityUri::parse(&r.id).expect("a seeded uri"))
            .collect()
    }
}

// ------------------------------------------------------- reference predicates

fn is_rule_head(row: &Row) -> bool {
    row.content_type == "source"
        && (row.source_language == "holon_rule" || row.source_language == "action")
}

/// The predicate the guard above states: my parent EXISTS and owns a
/// rule-head child.
fn is_program_via_hop(pop: &Population, subject: &Row) -> bool {
    if subject.content_type != "source" {
        return false;
    }
    if is_rule_head(subject) {
        return true;
    }
    let Some(parent) = pop.get(&subject.parent_id) else {
        return false;
    };
    pop.rows
        .iter()
        .any(|r| r.parent_id == parent.id && is_rule_head(r))
}

/// `is_program` as the PROFILE states it: `rule_sibling(parent_id)` is a
/// lookup keyed on the parent id, which never asks whether that parent is
/// itself a row.
fn is_program_via_rule_sibling(pop: &Population, subject: &Row) -> bool {
    if subject.content_type != "source" {
        return false;
    }
    if is_rule_head(subject) {
        return true;
    }
    pop.rows
        .iter()
        .any(|r| r.parent_id == subject.parent_id && is_rule_head(r))
}

// ------------------------------------------------------------ the compiled net

fn compiled_is_program() -> CompiledNet {
    let guard = is_program_guard();
    let classified = classify_guard(&guard, "op:block.move_block").expect("within the bounds");
    assert!(
        classified.residue.is_empty(),
        "the whole predicate must compile to arcs, residue left: {:?}",
        classified.residue
    );
    assert_eq!(
        classified.modes.len(),
        4,
        "two rule languages, each reachable directly and through the hop"
    );
    CompiledNet {
        transitions: vec![NetTransition::new(
            TransitionSource::Operation {
                entity: NetEntity::parse("block").expect("dotless"),
                op: "move_block".to_string(),
            },
            Analyzability::Analyzable,
            classified.modes,
            classified.residue,
        )],
    }
}

fn net_says_program(net: &CompiledNet, pop: &Population, subject: &str) -> bool {
    let uri = EntityUri::parse(subject).expect("a seeded uri");
    let mut offers = evaluate(net, pop, &ArcRelation::block(), &uri);
    assert_eq!(offers.len(), 1, "the fixture holds one transition");
    match offers.remove(0).offer {
        Offer::Enabled => true,
        Offer::Refused { .. } => false,
        Offer::Unknown { why } => {
            panic!("the predicate must be decidable from a marking, got Unknown: {why}")
        }
    }
}

// ------------------------------------------------------------------ boundaries

/// Ids are scheme-qualified the way the store holds them; the fixtures spell
/// the bare name.
fn uri(name: &str) -> String {
    if name.starts_with("block:") {
        name.to_string()
    } else {
        format!("block:{name}")
    }
}

fn row(id: &str, parent: &str, content_type: &str, language: &str) -> Row {
    Row {
        id: uri(id),
        parent_id: uri(parent),
        content_type: content_type.to_string(),
        source_language: language.to_string(),
    }
}

fn boundary_population() -> Population {
    Population {
        rows: vec![
            row("heading", NO_PARENT, "text", ""),
            row("rule-head", "heading", "source", "holon_rule"),
            row("trigger", "heading", "source", "holon_prql"),
            row("prose", "heading", "text", ""),
            row("legacy", NO_PARENT, "text", ""),
            row("legacy-head", "legacy", "source", "action"),
            row("legacy-trigger", "legacy", "source", "holon_prql"),
            row("plain-page", NO_PARENT, "text", ""),
            row("lonely-source", "plain-page", "source", "holon_prql"),
            row("solo", NO_PARENT, "text", ""),
            row("only-child-head", "solo", "source", "holon_rule"),
        ],
    }
}

#[test]
fn the_compiled_net_matches_the_hand_written_predicate_on_the_named_cases() {
    let net = compiled_is_program();
    let pop = boundary_population();
    let cases: [(&str, bool); 9] = [
        ("rule-head", true),
        ("trigger", true),
        ("legacy-head", true),
        ("legacy-trigger", true),
        ("lonely-source", false),
        ("prose", false),
        ("heading", false),
        ("only-child-head", true),
        ("solo", false),
    ];
    for (subject, expected) in cases {
        let reference = is_program_via_hop(&pop, pop.get(&uri(subject)).expect("a seeded row"));
        assert_eq!(
            reference, expected,
            "the reference transcription drifted for `{subject}`"
        );
        assert_eq!(
            net_says_program(&net, &pop, &uri(subject)),
            reference,
            "the compiled net disagrees for `{subject}`"
        );
    }
    println!("[hop-agreement] named cases: {}", cases.len());
}

/// A hop binds ONE token: a sibling that is a source block and a DIFFERENT
/// sibling that carries a rule language must not combine into a match.
///
/// This is the case the increment-0 spike went red on, kept as a regression.
#[test]
fn two_siblings_each_satisfying_one_half_do_not_enable_the_hop() {
    let pop = Population {
        rows: vec![
            row("home", NO_PARENT, "text", ""),
            row("subject", "home", "source", "holon_prql"),
            // Carries the language but is not a source block.
            row("half-a", "home", "text", "holon_rule"),
            // Is a source block but carries no rule language.
            row("half-b", "home", "source", "holon_prql"),
        ],
    };
    assert!(
        !is_program_via_hop(&pop, pop.get(&uri("subject")).expect("seeded")),
        "no single sibling is a rule head, so the reference says no"
    );
    assert!(
        !net_says_program(&compiled_is_program(), &pop, &uri("subject")),
        "the hop must bind one token; two halves on two entities is not a match"
    );
}

/// The profile's `rule_sibling(parent_id)` and the grammar's
/// `parent(child(...))` part company at the root: the lookup is keyed on the
/// parent id and never asks whether that parent is a row, while the hop
/// requires one.
///
/// Pinned, not papered over. `MoveGuard` must not be re-declared with
/// `parent(child(...))` until a sibling correlation keyed on `parent_id`
/// exists, or root-level rule machinery would silently stop being contained.
#[test]
fn the_hop_and_the_profile_lookup_diverge_at_the_root() {
    let pop = Population {
        rows: vec![
            row("root-head", NO_PARENT, "source", "holon_rule"),
            row("root-trigger", NO_PARENT, "source", "holon_prql"),
        ],
    };
    let subject = pop.get(&uri("root-trigger")).expect("seeded");
    assert!(
        is_program_via_rule_sibling(&pop, subject),
        "the profile lookup treats root blocks as siblings of one another"
    );
    assert!(
        !is_program_via_hop(&pop, subject),
        "the hop requires a parent ROW, which the root sentinel is not"
    );
    assert!(
        !net_says_program(&compiled_is_program(), &pop, &uri("root-trigger")),
        "the compiled net follows the grammar it was given, not the profile"
    );
}

// ------------------------------------------------------------------ generated

fn arb_population() -> impl Strategy<Value = Population> {
    let language = prop_oneof![
        Just("holon_rule"),
        Just("action"),
        Just("holon_prql"),
        Just(""),
    ];
    let content_type = prop_oneof![Just("source"), Just("text"), Just("image")];
    prop::collection::vec(
        (
            content_type.clone(),
            language.clone(),
            prop::collection::vec((content_type, language), 0..5),
        ),
        1..5,
    )
    .prop_map(|families| {
        let mut rows = Vec::new();
        for (p, (root_type, root_language, children)) in families.iter().enumerate() {
            let parent = format!("p{p}");
            // Roots vary too: a root that is itself rule machinery is the
            // shape the divergence test above isolates.
            rows.push(row(&parent, NO_PARENT, root_type, root_language));
            for (c, (content_type, language)) in children.iter().enumerate() {
                rows.push(row(&format!("p{p}c{c}"), &parent, content_type, language));
            }
        }
        Population { rows }
    })
}

/// Every block of every generated population, against the reference.
#[test]
fn the_compiled_net_agrees_with_the_hand_written_predicate() {
    let net = compiled_is_program();
    let checked = std::cell::Cell::new(0usize);
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases: 300,
        failure_persistence: None,
        ..ProptestConfig::default()
    });
    runner
        .run(&arb_population(), |pop| {
            for subject in &pop.rows {
                let reference = is_program_via_hop(&pop, subject);
                let compiled = net_says_program(&net, &pop, &subject.id);
                prop_assert_eq!(
                    compiled,
                    reference,
                    "disagreement on `{}` (content_type={}, language={}, parent={})",
                    subject.id,
                    subject.content_type,
                    subject.source_language,
                    subject.parent_id
                );
                checked.set(checked.get() + 1);
            }
            Ok(())
        })
        .expect("no disagreement between the compiled net and the hand-written predicate");
    let total = checked.get();
    println!("[hop-agreement] subjects compared: >= {total}");
    assert!(
        total >= 1000,
        "the differential oracle must see at least 1000 subjects, saw {total}"
    );
}
