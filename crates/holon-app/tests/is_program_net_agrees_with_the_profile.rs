//! The compiled net decides rule-machinery exactly as PRODUCTION does.
//!
//! The reference is not a restatement in this file: it is the profile
//! resolver's own `is_program` computed field
//! (`assets/default/types/block_profile.yaml:52`), evaluated through the same
//! `ProfileResolving` seam and the same row shape `MoveGuard::subject` builds
//! (`crates/holon-app/src/move_guard.rs:88-162`). That is the predicate the
//! shipped containment refusal rests on.
//!
//! The subject under test is the guard text a future `#[require]` would carry,
//! compiled by `holon_net::guards` and evaluated by `holon_net::enabledness`
//! over a marking built from the same forest.
//!
//! The two must agree on every block of every generated forest, INCLUDING at
//! the root: `rule_sibling(parent_id)` is keyed on the parent id and never
//! asks whether that parent is a row, which is why the guard says
//! `sibling(...)` — one hop keyed on `parent_id` — and not
//! `parent(child(...))`, which needs a parent row and so reaches nothing from
//! a root block.
//!
//! @pbt kind harness
//! @pbt covers is-program-net-agreement — the net's rule-machinery verdict
//! against the profile resolver's own computed field

use std::collections::HashMap;
use std::sync::Arc;

use holon_api::EntityUri;
use holon_api::arcs::ArcPlace;
use holon_api::arcs::ArcRelation;
use holon_api::block::Block;
use holon_api::pattern::Guard;
use holon_net::Analyzability;
use holon_net::CompiledNet;
use holon_net::NetEntity;
use holon_net::NetTransition;
use holon_net::TransitionSource;
use holon_net::enabledness::Offer;
use holon_net::enabledness::evaluate;
use holon_net::guards::classify_guard;
use holon_net::marking::Marking;
use holon_profiles::LiveEntitySpec;
use holon_profiles::ProfileResolver;
use holon_profiles::ProfileResolving;
use proptest::prelude::*;

/// `is_program`, written in the guard language.
///
/// `sibling(...)` reproduces `rule_sibling(parent_id)`: keyed on the column,
/// so two root blocks are siblings of one another. Both branches need
/// `content_type == "source"`, so it factors out of the disjunction.
const IS_PROGRAM: &str = "block.content_type == \"source\" and \
     (block.source_language == \"holon_rule\" or block.source_language == \"action\" or \
      sibling(block.content_type == \"source\" and \
      (block.source_language == \"holon_rule\" or block.source_language == \"action\")))";

// ------------------------------------------------------- the reference

/// The renderer's resolver, built the way a no-Turso session builds it.
fn resolver_from(blocks: &[Block]) -> Arc<dyn ProfileResolving> {
    let type_registry =
        holon_profiles::create_default_registry().expect("default TypeRegistry builds");
    let type_profiles = holon_profiles::type_profiles_from_registry(&type_registry);
    let mut live_entities = holon_profiles::LiveEntities::new();
    for spec in LiveEntitySpec::ALL.iter().copied() {
        live_entities.insert(
            spec.entity_name(),
            spec.live_data_from_blocks(blocks.iter()),
        );
    }
    let empty_profiles = holon_api::live_data::LiveData::new(
        Vec::new(),
        |_| Ok(String::new()),
        |_| anyhow::bail!("this fixture has no user profile source"),
    );
    Arc::new(ProfileResolver::with_type_profiles(
        empty_profiles,
        holon_api::UiInfo::default(),
        live_entities,
        HashMap::new(),
        type_profiles,
    ))
}

/// `is_program` for one block, through the production resolver — the same row
/// `MoveGuard::subject` builds.
fn profile_says_program(resolver: &dyn ProfileResolving, block: &Block) -> bool {
    let mut row = HashMap::new();
    row.insert(
        "id".to_string(),
        holon_api::Value::String(block.id.as_str().to_string()),
    );
    row.insert(
        "parent_id".to_string(),
        holon_api::Value::String(block.parent_id.as_str().to_string()),
    );
    row.insert(
        "content_type".to_string(),
        holon_api::Value::String(block.content_type.to_string()),
    );
    row.insert(
        "source_language".to_string(),
        match &block.source_language {
            Some(lang) => holon_api::Value::String(lang.to_string()),
            None => holon_api::Value::Null,
        },
    );
    let computed = resolver.resolve_computed_only(
        &row,
        &holon_api::render_requirements::RenderRequirements::none(),
    );
    match computed.get("is_program") {
        Some(holon_api::Value::Boolean(b)) => *b,
        // The evaluator's typed "unbound", which every renderer condition
        // treats as falsy — `MoveGuard` does the same.
        Some(holon_api::Value::Null) => false,
        other => panic!("`is_program` came back as {other:?}, not a flag"),
    }
}

// ------------------------------------------------------- the subject

/// The forest as a marking: cells by place, and one indexed lookup per hop.
struct Forest {
    blocks: Vec<Block>,
}

impl Forest {
    fn cell(&self, block: &Block, place: &str) -> Option<holon_api::Value> {
        match place {
            "block.id" => Some(holon_api::Value::String(block.id.as_str().to_string())),
            "block.parent_id" => Some(holon_api::Value::String(
                block.parent_id.as_str().to_string(),
            )),
            "block.content_type" => Some(holon_api::Value::String(block.content_type.to_string())),
            "block.source_language" => block
                .source_language
                .as_ref()
                .map(|l| holon_api::Value::String(l.to_string())),
            "block.content" => Some(holon_api::Value::String(block.content.clone())),
            other => panic!("the guard named a place this fixture has no column for: {other}"),
        }
    }

    fn get(&self, id: &EntityUri) -> Option<&Block> {
        self.blocks.iter().find(|b| b.id == *id)
    }
}

impl Marking for Forest {
    fn present(&self, _: &ArcRelation, entity: &EntityUri) -> bool {
        self.get(entity).is_some()
    }

    fn value(&self, place: &ArcPlace, entity: &EntityUri) -> Option<holon_api::Value> {
        self.get(entity)
            .and_then(|b| self.cell(b, &place.to_string()))
    }

    fn matching(&self, to: &ArcPlace, value: &holon_api::Value) -> Vec<EntityUri> {
        let place = to.to_string();
        self.blocks
            .iter()
            .filter(|b| self.cell(b, &place).as_ref() == Some(value))
            .map(|b| b.id.clone())
            .collect()
    }
}

fn compiled_is_program() -> CompiledNet {
    let guard = Guard::parse(IS_PROGRAM).expect("the is_program guard parses as written text");
    let classified =
        classify_guard(&guard, "op:block.move_block").expect("within the compiler's bounds");
    assert!(
        classified.residue.is_empty(),
        "the whole predicate must compile to arcs, residue left: {:?}",
        classified.residue
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

fn net_says_program(net: &CompiledNet, forest: &Forest, subject: &EntityUri) -> bool {
    let mut offers = evaluate(net, forest, &ArcRelation::block(), subject);
    assert_eq!(offers.len(), 1, "the fixture holds one transition");
    match offers.remove(0).offer {
        Offer::Enabled => true,
        Offer::Refused { .. } => false,
        Offer::Unknown { why } => {
            panic!("the predicate must be decidable from a marking, got Unknown: {why}")
        }
    }
}

// ------------------------------------------------------------ fixtures

/// The profile resolver's live-entity lookups need a reactor in scope even
/// when nothing awaits, so both tests run inside one.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime")
}

fn uri(s: &str) -> EntityUri {
    EntityUri::parse(s).expect("fixture uri")
}

fn text(id: &str, parent: EntityUri) -> Block {
    Block::new_text(uri(id), parent, "prose")
}

fn source(id: &str, parent: EntityUri, language: &str) -> Block {
    Block::new_source(uri(id), parent, language, "body")
}

/// The named shapes, including the two the composition would get wrong.
#[test]
fn the_net_matches_the_profile_on_the_named_shapes() {
    let rt = runtime();
    let _reactor = rt.enter();
    let blocks = vec![
        text("block:page", EntityUri::no_parent()),
        text("block:heading", uri("block:page")),
        source("block:rule-head", uri("block:heading"), "holon_rule"),
        source("block:trigger", uri("block:heading"), "holon_sql"),
        source("block:legacy-head", uri("block:heading"), "action"),
        text("block:prose", uri("block:heading")),
        text("block:lonely-page", EntityUri::no_parent()),
        source("block:lonely", uri("block:lonely-page"), "holon_sql"),
        // Rule machinery at TOP level: the sibling lookup keys on the root
        // sentinel like any other parent.
        source("block:top-head", EntityUri::no_parent(), "holon_rule"),
        source("block:top-trigger", EntityUri::no_parent(), "holon_sql"),
    ];
    let resolver = resolver_from(&blocks);
    let net = compiled_is_program();
    let forest = Forest {
        blocks: blocks.clone(),
    };

    for block in &blocks {
        let reference = profile_says_program(resolver.as_ref(), block);
        let compiled = net_says_program(&net, &forest, &block.id);
        assert_eq!(
            compiled, reference,
            "`{}` (content_type={}, language={:?}, parent={})",
            block.id, block.content_type, block.source_language, block.parent_id
        );
    }
    // The root pair is the case `parent(child(...))` would get wrong, so the
    // agreement above must not be vacuous there.
    assert!(
        profile_says_program(
            resolver.as_ref(),
            blocks
                .iter()
                .find(|b| b.id == uri("block:top-trigger"))
                .unwrap()
        ),
        "top-level rule machinery IS program, or the root case proves nothing"
    );
    println!("[is-program] named shapes agreeing: {}", blocks.len());
}

// ------------------------------------------------------------ generated

/// Forests of a few parents with a few children each, plus root-level blocks,
/// over the content types and languages the predicate turns on.
fn arb_forest() -> impl Strategy<Value = Vec<Block>> {
    let kind = prop_oneof![
        Just(None),
        Just(Some("holon_rule")),
        Just(Some("action")),
        Just(Some("holon_sql")),
    ];
    prop::collection::vec((kind.clone(), prop::collection::vec(kind, 0..4)), 1..4).prop_map(
        |families| {
            let mut blocks = Vec::new();
            for (p, (root_kind, children)) in families.iter().enumerate() {
                let parent = format!("block:p{p}");
                blocks.push(match root_kind {
                    Some(lang) => source(&parent, EntityUri::no_parent(), lang),
                    None => text(&parent, EntityUri::no_parent()),
                });
                for (c, kind) in children.iter().enumerate() {
                    let id = format!("block:p{p}c{c}");
                    blocks.push(match kind {
                        Some(lang) => source(&id, uri(&parent), lang),
                        None => text(&id, uri(&parent)),
                    });
                }
            }
            blocks
        },
    )
}

#[test]
fn the_net_agrees_with_the_profile_over_generated_forests() {
    let rt = runtime();
    let _reactor = rt.enter();
    let net = compiled_is_program();
    let checked = std::cell::Cell::new(0usize);
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases: 96,
        failure_persistence: None,
        ..ProptestConfig::default()
    });
    runner
        .run(&arb_forest(), |blocks| {
            let resolver = resolver_from(&blocks);
            let forest = Forest {
                blocks: blocks.clone(),
            };
            for block in &blocks {
                let reference = profile_says_program(resolver.as_ref(), block);
                let compiled = net_says_program(&net, &forest, &block.id);
                prop_assert_eq!(
                    compiled,
                    reference,
                    "`{}` (content_type={}, language={:?}, parent={})",
                    block.id.as_str(),
                    block.content_type.to_string(),
                    block.source_language.clone(),
                    block.parent_id.as_str()
                );
                checked.set(checked.get() + 1);
            }
            Ok(())
        })
        .expect("the net never contradicts the profile resolver");
    let total = checked.get();
    println!("[is-program] subjects compared: >= {total}");
    assert!(
        total >= 300,
        "the oracle must see at least 300 subjects, saw {total}"
    );
}
