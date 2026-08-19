#![cfg(feature = "pbt")]
//! Deterministic lock for the undo→redo mint-vs-remap divergence — the shrunk
//! keystone counterexample `[SplitBlock(c1, 0), UndoLastMutation, Redo]`.
//!
//! Without the fix this dies in the harness per-tick reconcile
//! (`composed/harness.rs`):
//!   per-tick reconcile: one synthetic per minted real id
//!   (syn=[], real=[block:<uuid>])  left: 0  right: 1
//!
//! Mechanism (observed, not inferred — instrumented reconcile dump):
//!   tick 1 SplitBlock  oracle mints `block::split-0`, SUT mints uuid U1;
//!                      resolver pairs split-0 -> U1.
//!   tick 2 Undo        oracle drops split-0, SUT DELETES U1. The resolver is
//!                      insert-only, so split-0 -> U1 survives — now dangling.
//!   tick 3 Redo        the oracle's redo snapshot restores the SAME label
//!                      `block::split-0`, while prod re-executes the stored
//!                      forward op and mints a FRESH uuid U2 (prod burns block
//!                      ids across undo — see `pop_undo_to_redo`). split-0
//!                      counts as already-mapped, so `synthetic` is empty while
//!                      `real_new` holds U2.
//!
//! It is an ORACLE/harness defect, not a duplicate-block prod defect: the SUT
//! block count goes 28 -> 27 -> 28 across the three ticks, matching the
//! oracle's exactly, and U1 is gone. `inv-blocks-match-ref` (set equality
//! between the SUT and the resolver-resolved oracle) runs on every tick here,
//! so this test passing is itself the proof that the split tail exists exactly
//! once in the SUT — a masked duplicate would fail it.

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_pbt_core::fixture::Fixture;
use proptest::test_runner::Config;
use proptest_state_machine::StateMachineTest;

fn replay(line: &str) {
    let case: Fixture<E2ETransition> =
        serde_json::from_str(line).expect("reproducer case must parse");
    let config = Config {
        verbose: 1,
        ..Config::default()
    };
    let initial_state = wide_e2e_ref();
    ComposedSut::<WideE2E>::test_sequential(config, initial_state, case.transitions, None);
}

#[test]
fn split_undo_redo_reconciles() {
    let line = r#"{"name": "split-undo-redo-reconcile", "description": "redo re-mints the split tail under a fresh uuid; the resolver must retire the burned pair and re-pair the oracle's synthetic", "transitions": [{"SplitBlock": {"block_id": "block:c1", "position": 0}}, {"UndoLastMutation": null}, {"Redo": null}]}"#;
    replay(line);
}
