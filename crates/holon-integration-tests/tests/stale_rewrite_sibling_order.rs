#![cfg(feature = "pbt")]
//! Deterministic minimized reproducer for the EXTERNAL_RACES sibling-order
//! divergence (Task #23 — the final EXTERNAL_RACES flip blocker).
//!
//! Shrunk keystone counterexample (armed `HOLON_PBT_EXTERNAL_RACES=1`):
//!   [CreateDocument, CreateBlockUnderFocus, BulkExternalAdd, SplitBlock,
//!    PinBlock, SplitBlock, StaleExternalRewrite]
//! Without the fix this replay does NOT reach any org comparison: it dies
//! earlier, on the harness reconcile assertion at
//! `crates/holon-integration-tests/src/pbt/composed/harness.rs:640`:
//!   per-tick reconcile: one synthetic per minted real id
//!   (syn=[], real=[block:<uuid>])  left: 0  right: 1
//! i.e. a mint-vs-remap divergence. So this repro proves the fix corrects
//! reconcile mint/remap behavior; it does NOT by itself exhibit the
//! sibling-order swap in the org projection — the keystone covers that.
//!
//! Essence: splitting `c1` ("c1") at position 0 twice produces two
//! EMPTY-content siblings — `c1` (now "") and a split-minted block (also ""). A
//! `StaleExternalRewrite` (id-stripped reingest of the doc's own current
//! content) then reconciles every block 1:1 by position (`tiered_match` remaps
//! all), yet the SUT's org projection lands the two empties in the WRONG order.
//!
//! This file replays the EXACT shrunk sequence AND a hand-minimized core
//! through the production keystone harness. It is RED-for-the-right-reason
//! until the ordering bug is fixed, then a permanent green lock.

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_pbt_core::fixture::Fixture;
use proptest::test_runner::Config;
use proptest_state_machine::StateMachineTest;

fn replay(line: &str) {
    // Arm the external-rewrite transition's SUT/ref paths (belt-and-suspenders:
    // the hand-authored replay bypasses the generator gate, but any env-guarded
    // code inside the transition must see it armed too).
    unsafe { std::env::set_var("HOLON_PBT_EXTERNAL_RACES", "1") };
    let case: Fixture<E2ETransition> =
        serde_json::from_str(line).expect("reproducer case must parse");
    let config = Config {
        verbose: 1,
        ..Config::default()
    };
    let initial_state = wide_e2e_ref();
    ComposedSut::<WideE2E>::test_sequential(config, initial_state, case.transitions, None);
}

/// Hand-minimized CORE: just the two position-0 splits of `c1` + the stale
/// rewrite. If this reproduces, the doc-create / bulk-add / pin steps in the
/// shrunk sequence are incidental.
#[test]
fn stale_rewrite_empty_sibling_order_core() {
    let line = r#"{"name": "stale-rewrite-empty-sibling-order-core", "description": "two position-0 splits of c1 make two empty siblings; StaleExternalRewrite must keep their order (inv-blocks-match-ref/org)", "transitions": [{"SplitBlock": {"block_id": "block:c1", "position": 0}}, {"SplitBlock": {"block_id": "block:c1", "position": 0}}, {"StaleExternalRewrite": {"doc_uri": "block:structural-page"}}]}"#;
    replay(line);
}

/// The FULL shrunk keystone counterexample, verbatim (minus the BulkExternalAdd
/// block payload, which targets the unrelated `doc_0` — re-added in the
/// `_with_bulk` case if the core does not reproduce).
#[test]
fn stale_rewrite_empty_sibling_order_full_shrink() {
    let line = r#"{"name": "stale-rewrite-empty-sibling-order-full", "description": "verbatim shrunk keystone counterexample for the EXTERNAL_RACES /org sibling-order red", "transitions": [{"CreateDocument": {"file_name": "doc_0.org"}}, {"CreateBlockUnderFocus": {"content": "tzsvx", "id": "block:gen-0"}}, {"SplitBlock": {"block_id": "block:c1", "position": 0}}, {"PinBlock": {"region": "right_sidebar", "block_id": "block:c2"}}, {"SplitBlock": {"block_id": "block:c1", "position": 0}}, {"StaleExternalRewrite": {"doc_uri": "block:structural-page"}}]}"#;
    replay(line);
}
