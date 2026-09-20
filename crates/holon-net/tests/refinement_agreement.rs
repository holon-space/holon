//! An arc's refinement decides a conjunct exactly as the guard language does.
//!
//! An arc names a place, `relation.field`, and nothing finer. A predicate that
//! also names something INSIDE that field — the key of a property — must carry
//! it on the refinement, or two different predicates compile to one arc and
//! the net answers one for the other.
//!
//! The general law is at the bottom: over generated conjuncts and rows,
//! whenever the net decides, it agrees with `holon_pattern`'s own 2-valued
//! verdict. The named case above it is the property-key hole, kept as a
//! regression.

use std::collections::HashMap;

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
use holon_pattern::pattern::InMemoryWorld;
use holon_pattern::pattern::Operand;
use holon_pattern::pattern::Pattern;
use holon_pattern::pattern::Subject;
use holon_pattern::pattern::WorldBlock;
use proptest::prelude::*;

const SUBJECT: &str = "block:subject";

/// One block, exposed to both evaluators: as a `WorldBlock` to the guard
/// language and as a `Marking` to the net.
#[derive(Debug, Clone)]
struct OneBlock {
    name: String,
    properties: HashMap<String, Value>,
    tags: Vec<String>,
    columns: HashMap<String, Value>,
}

impl OneBlock {
    fn empty() -> Self {
        OneBlock {
            name: String::new(),
            properties: HashMap::new(),
            tags: Vec::new(),
            columns: HashMap::new(),
        }
    }

    fn with_property(mut self, key: &str, value: &str) -> Self {
        self.properties
            .insert(key.to_string(), Value::String(value.to_string()));
        self
    }

    fn world(&self) -> InMemoryWorld {
        InMemoryWorld::new(
            vec![WorldBlock {
                id: SUBJECT.to_string(),
                name: self.name.clone(),
                parent_id: None,
                properties: self.properties.clone(),
                tags: self.tags.clone(),
                columns: self.columns.clone(),
            }],
            "2026-09-20",
        )
    }
}

impl Marking for OneBlock {
    fn present(&self, _: &ArcRelation, entity: &EntityUri) -> bool {
        entity.as_str() == SUBJECT
    }

    fn value(&self, place: &ArcPlace, entity: &EntityUri) -> Option<Value> {
        if entity.as_str() != SUBJECT {
            return None;
        }
        match place.to_string().as_str() {
            "block.content" => Some(Value::String(self.name.clone())),
            "block.properties" => Some(Value::Object(self.properties.clone())),
            "block.tags" => Some(Value::Array(
                self.tags.iter().cloned().map(Value::String).collect(),
            )),
            other => {
                let column = other.strip_prefix("block.").expect("a block place");
                self.columns.get(column).cloned()
            }
        }
    }

    fn matching(&self, _: &ArcPlace, _: &Value) -> Vec<EntityUri> {
        // One block, no parent: nothing to hop to.
        Vec::new()
    }
}

/// What the net says about a one-conjunct block guard, or `None` when it
/// declines to decide.
fn net_verdict(body: &Pattern, block: &OneBlock) -> Option<bool> {
    let guard = Guard {
        subject: Subject::Block,
        body: body.clone(),
    };
    let classified = classify_guard(&guard, "op:block.probe").ok()?;
    if !classified.residue.is_empty() {
        return None;
    }
    let net = CompiledNet {
        transitions: vec![NetTransition::new(
            TransitionSource::Operation {
                entity: NetEntity::parse("block").expect("dotless"),
                op: "probe".to_string(),
            },
            Analyzability::Analyzable,
            classified.modes,
            classified.residue,
        )],
    };
    let subject = EntityUri::parse(SUBJECT).expect("a uri");
    let mut offers = evaluate(&net, block, &ArcRelation::block(), &subject);
    match offers.remove(0).offer {
        Offer::Enabled => Some(true),
        Offer::Refused { .. } => Some(false),
        Offer::Unknown { .. } => None,
    }
}

fn guard_verdict(body: &Pattern, block: &OneBlock) -> bool {
    let guard = Guard {
        subject: Subject::Block,
        body: body.clone(),
    };
    guard
        .evaluate(&block.world())
        .expect("a block guard evaluates")
        .enabled()
}

fn property(key: &str, value: &str) -> Pattern {
    Pattern::Field {
        field: FieldRef::Property(key.to_string()),
        op: CmpOp::Eq,
        rhs: Operand::Lit(Value::String(value.to_string())),
    }
}

/// The verifier's exact counterexample: the key is part of the predicate.
#[test]
fn a_property_conjunct_is_decided_by_its_key() {
    let block = OneBlock::empty().with_property("b", "x");
    assert!(
        !guard_verdict(&property("a", "x"), &block),
        "the guard language reads key `a`, which the block does not hold"
    );
    assert_eq!(
        net_verdict(&property("a", "x"), &block),
        Some(false),
        "the net must read key `a` too, not merely the properties place"
    );
    assert_eq!(
        net_verdict(&property("b", "x"), &block),
        Some(true),
        "and it must still decide the key the block DOES hold"
    );
}

/// Two keys must not compile to one arc.
#[test]
fn two_property_keys_compile_to_different_arcs() {
    let of = |key: &str| {
        classify_guard(
            &Guard {
                subject: Subject::Block,
                body: property(key, "x"),
            },
            "op:block.probe",
        )
        .expect("compiles")
    };
    assert_ne!(
        of("a"),
        of("b"),
        "two guards with different meaning compiled to one net"
    );
}

/// A conjunction over two keys of one place must be satisfiable — both arcs
/// land on `block.properties`, and only the keys tell them apart.
#[test]
fn a_conjunction_over_two_property_keys_can_hold() {
    let block = OneBlock::empty()
        .with_property("a", "x")
        .with_property("b", "y");
    let body = Pattern::And(vec![property("a", "x"), property("b", "y")]);
    assert!(guard_verdict(&body, &block));
    assert_eq!(net_verdict(&body, &block), Some(true));
}

/// A `block.properties` place holding something that is not a map is a shape
/// error at the marking boundary. Answering it `false` would be a confident
/// refusal built on malformed data — the project's "silently degrades to look
/// fine" case — so the net declines instead.
#[test]
fn a_properties_place_that_is_not_a_map_is_unknown_not_refused() {
    struct MalformedBag;
    impl Marking for MalformedBag {
        fn present(&self, _: &ArcRelation, _: &EntityUri) -> bool {
            true
        }
        fn value(&self, _: &ArcPlace, _: &EntityUri) -> Option<Value> {
            Some(Value::String("a=x".to_string()))
        }
        fn matching(&self, _: &ArcPlace, _: &Value) -> Vec<EntityUri> {
            Vec::new()
        }
    }
    let guard = Guard {
        subject: Subject::Block,
        body: property("a", "x"),
    };
    let classified = classify_guard(&guard, "op:block.probe").expect("compiles");
    let net = CompiledNet {
        transitions: vec![NetTransition::new(
            TransitionSource::Operation {
                entity: NetEntity::parse("block").expect("dotless"),
                op: "probe".to_string(),
            },
            Analyzability::Analyzable,
            classified.modes,
            classified.residue,
        )],
    };
    let subject = EntityUri::parse(SUBJECT).expect("a uri");
    let offer = evaluate(&net, &MalformedBag, &ArcRelation::block(), &subject)
        .remove(0)
        .offer;
    assert!(
        matches!(&offer, Offer::Unknown { why } if why.contains("block.properties")),
        "a malformed bag must not produce a confident verdict: {offer:?}"
    );
}

// ------------------------------------------------------------------ the law

fn arb_block() -> impl Strategy<Value = OneBlock> {
    (
        "[ab]{0,2}",
        prop::collection::hash_map("[ab]", "[xy]", 0..3),
        prop::collection::vec("[pq]", 0..2),
        prop::collection::hash_map("[cd]", "[xy]", 0..3),
    )
        .prop_map(|(name, properties, tags, columns)| OneBlock {
            name,
            properties: properties
                .into_iter()
                .map(|(k, v)| (k, Value::String(v)))
                .collect(),
            tags,
            columns: columns
                .into_iter()
                .map(|(k, v)| (k, Value::String(v)))
                .collect(),
        })
}

/// Conjuncts over the shapes a one-block world can decide. `c`/`d` are
/// declared block columns; `a`/`b` are property keys.
fn arb_conjunct() -> impl Strategy<Value = Pattern> {
    prop_oneof![
        ("[ab]", "[xy]").prop_map(|(k, v)| property(&k, &v)),
        "[xy]".prop_map(|v| Pattern::Field {
            field: FieldRef::Name,
            op: CmpOp::Eq,
            rhs: Operand::Lit(Value::String(v)),
        }),
        // A block-subject column comparison, which `arb_block` fills.
        ("[cd]", "[xy]").prop_map(|(k, v)| Pattern::Field {
            field: FieldRef::Column {
                relation: "block".to_string(),
                name: k,
            },
            op: CmpOp::Eq,
            rhs: Operand::Lit(Value::String(v)),
        }),
        ("[ab]", "[xy]").prop_map(|(k, v)| Pattern::Not(Box::new(property(&k, &v)))),
        "[pq]".prop_map(Pattern::HasTag),
    ]
}

fn arb_body() -> impl Strategy<Value = Pattern> {
    arb_conjunct().prop_recursive(2, 6, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 1..3).prop_map(Pattern::And),
            prop::collection::vec(inner, 1..3).prop_map(Pattern::Or),
        ]
    })
}

/// The general form of the property-key hole: whenever the net decides a
/// guard, it decides it the way the guard language does. A refinement that
/// drops any part of its conjunct reds here, not only the property key.
#[test]
fn whenever_the_net_decides_it_agrees_with_the_guard_language() {
    let decided = std::cell::Cell::new(0usize);
    let total = std::cell::Cell::new(0usize);
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases: 400,
        failure_persistence: None,
        ..ProptestConfig::default()
    });
    runner
        .run(&(arb_body(), arb_block()), |(body, block)| {
            total.set(total.get() + 1);
            if let Some(net) = net_verdict(&body, &block) {
                decided.set(decided.get() + 1);
                let guard = guard_verdict(&body, &block);
                prop_assert_eq!(
                    net,
                    guard,
                    "net and guard disagree on {:?} over {:?}",
                    body,
                    block
                );
            }
            Ok(())
        })
        .expect("the net never contradicts the guard language");
    println!(
        "[refinement-agreement] decided {} of {} generated conjunct/row pairs",
        decided.get(),
        total.get()
    );
    // A law nothing satisfies proves nothing. The generator deliberately draws
    // shapes the net must NOT decide (negation, tag membership), so the floor
    // is an absolute count rather than a fraction: 168 of 400 measured, and a
    // collapse to `Unknown` would fall far below 100.
    assert!(
        decided.get() >= 100,
        "the net decided only {} of {} cases; the law has gone vacuous",
        decided.get(),
        total.get()
    );
}
