//! Storage-consistency slice PBT — Phase 8 deliverable.
//!
//! Exercises **storage-layer invariants only** with a reduced transition set,
//! catching Turso/Loro matview-drift bugs in seconds rather than minutes.
//!
//! # SUT
//!
//! Reuses `E2ESut<SqlOnly>` — the existing slim profile that skips the
//! GPUI / reactive engine stack entirely.
//!
//! # Transition set
//!
//! Storage-layer transitions only (block tree, Loro, Turso, CDC). UI /
//! navigation / focus transitions excluded. `WriteOrgFile` is filtered to
//! regular files only — the `index.org` variants trigger an
//! `assert_cdc_quiescent` race during StartApp (known E2E infra race —
//! fix in Phase 9).
//!
//! # Invariants
//!
//! - `InvLoroNoErrors`
//! - `InvBlockTagsReferencesExist`
//!
//! See `crates/holon-integration-tests/src/pbt/slice.rs` for the
//! `declare_pbt_slice!` macro that drives the boilerplate.
//!
//! # Deferred invariants (with reasons)
//!
//! - `InvBlockIdsMatchRef` — `SutSqlProjection::all_block_ids()` returns ALL
//!   `block_raw` rows including seeds, while `ref_.all_non_seed_block_ids()`
//!   excludes them. Wire seed-aware variant in Phase 9.
//! - `InvTaskStateStorageCoherence` — `SutLoroTaskState::loro_task_state_of`
//!   is `unimplemented!()` on `E2ESut`. Including it would panic.

#![cfg(feature = "pbt")]

use std::time::Instant;

use proptest::strategy::Strategy;
use proptest_state_machine::{ReferenceStateMachine, StateMachineTest};

use holon_integration_tests::declare_pbt_slice;
use holon_integration_tests::pbt::enable_atomic_editor_if_unset;
use holon_integration_tests::pbt::transitions::{
    AddPeer, BulkExternalAdd, DeleteBackward, Indent, JoinBlock, MergeFromPeer, MoveDown, MoveUp,
    Outdent, PeerCharEdit, PeerEdit, SplitBlock, SyncWithPeer, TypeChars,
};

declare_pbt_slice! {
    test_fn: storage_consistency_pbt,
    variant_ref: holon_integration_tests::pbt::VariantRef<holon_integration_tests::pbt::SqlOnly>,
    inner_sut: holon_integration_tests::pbt::E2ESut<holon_integration_tests::pbt::SqlOnly>,
    transitions: [
        preset lifecycle,
        BulkExternalAdd,
        AddPeer,
        PeerEdit,
        PeerCharEdit,
        SyncWithPeer,
        MergeFromPeer,
        SplitBlock,
        JoinBlock,
        Indent,
        Outdent,
        MoveUp,
        MoveDown,
        TypeChars,
        DeleteBackward,
    ],
    invariants: [preset storage],
    cases: 16,
    max_shrink_iters: 20,
    steps: 1..10,
}

/// Microbenchmark: wall-clock for the storage-consistency slice.
///
/// Marked `#[ignore]` by default. To collect approximate per-transition timing,
/// run the main PBT with `--nocapture`.
#[test]
#[ignore = "raw loop bypasses CDC quiescence — use the main storage_consistency_pbt for timing"]
fn storage_consistency_microbenchmark() {
    enable_atomic_editor_if_unset();

    let cases = 64_u32;
    let steps_per_case = 30_usize;

    let mut total_transitions = 0_usize;
    let start = Instant::now();
    let mut runner = proptest::test_runner::TestRunner::default();

    for _ in 0..cases {
        let state_tree = match StorageConsistencyPbtMachine::init_state().new_tree(&mut runner) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let mut ref_state = state_tree.current();

        let sut = StorageConsistencyPbtSut::init_test(&ref_state);
        let mut sut = sut;

        for _ in 0..steps_per_case {
            let strategy = StorageConsistencyPbtMachine::transitions(&ref_state);
            let transition = match strategy.new_tree(&mut runner) {
                Ok(t) => t.current(),
                Err(_) => break,
            };
            if !StorageConsistencyPbtMachine::preconditions(&ref_state, &transition) {
                break;
            }
            ref_state = StorageConsistencyPbtMachine::apply(ref_state, &transition);
            sut =
                <StorageConsistencyPbtSut as StateMachineTest>::apply(sut, &ref_state, transition);
            <StorageConsistencyPbtSut as StateMachineTest>::check_invariants(&sut, &ref_state);
            total_transitions += 1;
        }
    }

    let elapsed = start.elapsed();
    let per_transition_us = elapsed.as_micros() as f64 / total_transitions.max(1) as f64;
    println!("===== storage_consistency_pbt microbenchmark =====");
    println!("Cases:                {}", cases);
    println!("Transitions applied:  {}", total_transitions);
    println!("Total wall:           {:?}", elapsed);
    println!("Per transition:       {:.1} µs", per_transition_us);
    println!("==================================================");
}
