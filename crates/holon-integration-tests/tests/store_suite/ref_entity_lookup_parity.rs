//! Prod/oracle parity for the Rhai entity-lookup functions the bundled `block`
//! profile's computed fields call (`query_source`, `rule_sibling`).
//!
//! The SUT evaluates those computed fields on the ProfileResolver's engine,
//! which carries the lookups registered from live entities. The oracle
//! evaluates the SAME profile, so an oracle engine without the lookups leaves
//! every lookup-dependent field (`has_query_source`, `is_program`) at `Null`
//! and floods the keystone log with `Function not found: query_source`.
//!
//! @pbt kind harness
//! @pbt covers ref-entity-lookup-parity — oracle-side Rhai engine carries the
//! same entity-lookup functions as the production ProfileResolver

#![cfg(feature = "pbt")]

use std::collections::HashMap;

use holon_api::EntityUri;
use holon_api::QueryLanguage;
use holon_api::SourceLanguage;
use holon_api::Value;
use holon_api::block::Block;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::reference_state::ReferenceState;
use holon_integration_tests::pbt::reference_state::block_to_data_row;

/// Insert `parent` plus a source child of `language` under it, and return both.
fn with_source_child(
    state: &mut ReferenceState,
    parent_id: &str,
    language: &str,
) -> (Block, Block) {
    let parent = EntityUri::block(parent_id);
    let child = EntityUri::block(&format!("{parent_id}-src"));
    let parent_block = Block::new_text(parent.clone(), EntityUri::no_parent(), "heading");
    let child_block = Block::new_source(child.clone(), parent.clone(), language, "from block");
    state
        .domain
        .block_state
        .blocks
        .insert(parent, parent_block.clone());
    state
        .domain
        .block_state
        .blocks
        .insert(child, child_block.clone());
    (parent_block, child_block)
}

/// Evaluate the bundled block profile's computed fields on the oracle engine.
fn computed_fields(state: &ReferenceState, block: &Block) -> HashMap<String, Value> {
    let profile = state
        .domain
        .seed_profile
        .as_ref()
        .expect("oracle carries the bundled block profile");
    profile.compute_fields_only(&block_to_data_row(block), &state.profile_engine())
}

/// `has_query_source` = "this block owns a query-source child and is not rule
/// machinery" — a `query_source(id)` lookup the oracle must be able to answer.
#[test]
fn oracle_computes_has_query_source_for_query_source_owner() {
    let mut state = wide_e2e_ref();
    let (owner, _) = with_source_child(
        &mut state,
        "qs-parity-owner",
        &SourceLanguage::Query(QueryLanguage::HolonPrql).to_string(),
    );

    assert_eq!(
        computed_fields(&state, &owner).get("has_query_source"),
        Some(&Value::Boolean(true)),
        "oracle must see the query-source child through the `query_source` lookup"
    );
}

/// A block with no source child has no query source — the lookup must answer
/// `false`, never `Null`.
#[test]
fn oracle_computes_has_query_source_false_without_child() {
    let mut state = wide_e2e_ref();
    let plain = EntityUri::block("qs-parity-plain");
    let plain_block = Block::new_text(plain.clone(), EntityUri::no_parent(), "heading");
    state
        .domain
        .block_state
        .blocks
        .insert(plain, plain_block.clone());

    assert_eq!(
        computed_fields(&state, &plain_block).get("has_query_source"),
        Some(&Value::Boolean(false)),
        "a childless block resolves the lookup to a definite `false`"
    );
}

/// The rule head's own parent owns a rule-head child, so `rule_sibling(id)`
/// suppresses `has_query_source` — the C-revised ruling that keeps a rule
/// trigger off the display-query path.
#[test]
fn oracle_suppresses_has_query_source_for_rule_machinery() {
    let mut state = wide_e2e_ref();
    let (owner, _) = with_source_child(&mut state, "rule-parity-owner", "holon_rule");

    assert_eq!(
        computed_fields(&state, &owner).get("has_query_source"),
        Some(&Value::Boolean(false)),
        "a rule head's parent must not be treated as a query page"
    );
}

/// The engine is memoized per source-block fingerprint, so grafting a
/// query-source child between two resolutions must flip the answer — a stale
/// engine would silently weaken every lookup-dependent prediction.
#[test]
fn oracle_engine_tracks_source_block_mutations() {
    let mut state = wide_e2e_ref();
    let parent = EntityUri::block("qs-parity-late");
    let parent_block = Block::new_text(parent.clone(), EntityUri::no_parent(), "heading");
    state
        .domain
        .block_state
        .blocks
        .insert(parent.clone(), parent_block.clone());

    assert_eq!(
        computed_fields(&state, &parent_block).get("has_query_source"),
        Some(&Value::Boolean(false)),
        "before the graft the block owns no query source"
    );

    let child = EntityUri::block("qs-parity-late-src");
    state.domain.block_state.blocks.insert(
        child.clone(),
        Block::new_source(
            child,
            parent,
            &SourceLanguage::Query(QueryLanguage::HolonPrql).to_string(),
            "from block",
        ),
    );

    assert_eq!(
        computed_fields(&state, &parent_block).get("has_query_source"),
        Some(&Value::Boolean(true)),
        "the memoized engine must be invalidated by the grafted query source"
    );
}

/// `is_program` clause (b): a source block whose parent owns a rule head is the
/// rule's trigger sibling — a `rule_sibling(parent_id)` lookup.
#[test]
fn oracle_computes_is_program_for_rule_trigger_sibling() {
    let mut state = wide_e2e_ref();
    let (_, rule_head) = with_source_child(&mut state, "rule-sib-owner", "holon_rule");
    let trigger = EntityUri::block("rule-sib-owner-trigger");
    let trigger_block = Block::new_source(
        trigger.clone(),
        rule_head.parent_id.clone(),
        &SourceLanguage::Query(QueryLanguage::HolonPrql).to_string(),
        "from block",
    );
    state
        .domain
        .block_state
        .blocks
        .insert(trigger, trigger_block.clone());

    assert_eq!(
        computed_fields(&state, &trigger_block).get("is_program"),
        Some(&Value::Boolean(true)),
        "the trigger sibling of a rule head is program machinery"
    );
}
