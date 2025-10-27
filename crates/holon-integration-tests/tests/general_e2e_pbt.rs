//! General-Purpose Property-Based E2E Test
//!
//! This is the test entry point. The state machine, SUT, generators, and types
//! live in `src/pbt/` so they can be reused by other harnesses (e.g. Flutter FFI).
//!
//! # Coverage Roadmap
//!
//! Planned additions ranked by bug-catching potential. Check off as implemented.
//!
//! ## Tier 1 — High impact, catches real production bugs
//!
//! - [x] **Undo/Redo transitions**: `UndoLastMutation` and `Redo` transitions exercising
//!       `BackendEngine::undo()/redo()`. Reference model uses `BlockState` snapshot-based
//!       undo stack. Catches state corruption in the operation journal.
//!
//! - [ ] **Profile variant switching**: `SwitchVariant` transition calling
//!       `WatchHandle::set_variant()` on an active watch. Verify the re-rendered
//!       `UiEvent::Structure` matches the new variant. Reference model already tracks
//!       `active_profiles`.
//!
//! - [x] **ViewModel structure assertions**: Strengthen inv10 beyond "root != error".
//!       Compare widget type at root (columns/list/table) against render expression.
//!       Assert entity IDs in tree match query result set. Use existing helpers:
//!       `tree_diff()`, `is_ordered_subset()`, `assert_display_trees_match()`.
//!       Implemented: 10c (error count), 10d (root widget type vs RenderExpr),
//!       10e (entity ID ordering), 10f (decompiled row data),
//!       10g (EditableText trigger presence).
//!       ReferenceState tracks `RenderExpr` per render source block.
//!
//! - [x] **Slash command trigger pipeline**: `TriggerSlashCommand` transition exercising
//!       the full three-tier input model: check_triggers() → ViewEventHandler →
//!       CommandMenuController → select "delete" → execute operation. Validates triggers
//!       are present on EditableText nodes and the shared menu logic works correctly.
//!
//! - [x] **Text edit via ViewModel**: `EditViaViewModel` transition exercising the
//!       Tier 3 TextSync path: render → ViewModel → verify triggers present → verify
//!       normal text doesn't trigger → ViewEvent::TextSync → ViewEventHandler returns
//!       MenuAction::Execute with set_field params → dispatch operation.
//!
//! - [ ] **Cross-document block Move**: Move blocks between documents (re-parent across
//!       doc boundaries). Exercises document_id rewriting, org file sync across two
//!       files, and CDC propagation to multiple watches simultaneously.
//!
//! ## Tier 2 — Medium effort, catches subtle bugs
//!
//! - [ ] **Delete-then-navigate**: Delete a block that is the current navigation focus
//!       target. The matview chain `navigation_cursor → focus_roots` must handle this
//!       gracefully (not panic, not show stale data).
//!
//! - [ ] **Concurrent multi-document external edits**: Write two `.org` files in one
//!       transition. Tests file watcher's multi-event processing and OrgSyncController's
//!       per-document echo suppression.
//!
//! - [ ] **Error recovery in watch_ui**: Mutate a render source block to contain garbage
//!       DSL → verify `watch_ui` emits Structure with error widget (not panic) → fix the
//!       render source → verify valid Structure is emitted. Tests error→recovery path.
//!
//! ## Tier 3 — Lower effort, defensive value
//!
//! - [ ] **Property round-trip with special characters**: Generate `org_properties` with
//!       unicode, colons, newlines, JSON-like content. The serialization path
//!       (org→parse→SQL→cache→render→org) has had bugs (CacheEventSubscriber fix).
//!
//! - [ ] **Source language near-miss discrimination**: Generate blocks with source_language
//!       values close to reserved ones ("prql_custom", "sql_view"). Verify
//!       `load_root_layout_block()` doesn't false-match them as query/render blocks.
//!
//! - [ ] **Custom TODO keyword sets**: Test with `#+TODO: TODO REVIEW | DONE CANCELLED`.
//!       Exercises `TaskState::from_keyword_with_done_list()` which has had bugs with
//!       missing done-keywords.
//!
//! ## Tier 4 — Action watcher (query-triggered operations)
//!
//! Prerequisite: generalize block mutation transitions to `CreateBlock { source: MutationSource }`
//! where `MutationSource` is `Ui | Org | Loro | Action`. Reference model applies the same
//! block state change regardless of source; SUT dispatches differently. Invariants are identical.
//!
//! - [ ] **Action discovery and execution**: Write an org file with a query+action pair
//!       (e.g., `SELECT 'test' as name` + `block.create(#{parent_id: ..., name: col("name")})`).
//!       After StartApp, invariant checks that the action-created block exists in DB.
//!       Reference model predicts creation based on active action pairs.
//!
//! - [ ] **Action + user delete interaction**: User deletes an action-created block.
//!       Verify it does NOT reappear (volatile query, CDC doesn't re-fire).
//!       For table-backed triggers: verify it DOES reappear if trigger row still matches.
//!
//! - [ ] **Dynamic discovery**: Add action blocks via WriteOrgFile mid-test.
//!       Streaming discovery matview should pick up new pairs without restart.
//!       Reference model tracks active_actions and updates predictions.
//!
//! - [ ] **Idempotency under concurrent mutation**: Action fires while user mutates
//!       the same parent block. INSERT OR IGNORE prevents duplicates. Invariant:
//!       no constraint violations, no duplicate (parent_id, name) pairs.
//!
//! - [ ] **Action cascade guard**: Action creates block matching ANOTHER action's trigger.
//!       INSERT OR IGNORE prevents infinite loops. Invariant: finite block count,
//!       bounded action execution count per transition.

// # Fast regression replay
//
// Each test variant runs `cases: 8` random sequences (~25 min each at the
// time of writing). To validate a fix against a previously-failing seed
// without paying for a full random sweep, set `PROPTEST_CASES=1` — proptest
// replays every `cc <hash>` line from `general_e2e_pbt.proptest-regressions`
// first, *then* runs `cases` random ones. With `PROPTEST_CASES=1`, the
// persisted seeds still replay (cc lines are not gated by `cases`), and the
// random run after them is just one case. Combined with
// `PROPTEST_MAX_SHRINK_ITERS=0` to disable re-shrink, this is the fastest
// sanity-check loop available without adding new tests.
//
// # Verbose logging
//
// PBT breadcrumbs (`[apply]`, `[BulkExternalAdd]`, `[CUSTOMPROP-TRACE …]`,
// etc.) have moved to `tracing::trace!`. They emit no output by default;
// initialize a subscriber and set `RUST_LOG=trace` (or
// `holon_integration_tests=trace`) to opt in.

use std::path::PathBuf;

use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

use holon_integration_tests::pbt::{CrossExecutor, E2ESut, Full, SqlOnly};

fn pbt_config() -> ProptestConfig {
    // Default of 50 (was 10) so persisted regression seeds are shrunk to a
    // more minimal form by default. PBT failures are expensive to reproduce
    // (~25 min per case at the time of writing); a slightly higher shrink
    // budget pays for itself the next time someone replays the seed.
    let max_shrink = std::env::var("PROPTEST_MAX_SHRINK_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);
    // Pin the regressions file to an explicit path. The default
    // `SourceParallel` mode tries to locate `lib.rs`/`main.rs` adjacent to
    // the test source and prints a misleading "failed to find lib.rs or
    // main.rs" warning when it can't (proptest_state_machine generates
    // tests that confuse the heuristic). All three variants share one
    // file by design — they exercise the same state machine.
    let regressions = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("general_e2e_pbt.proptest-regressions");
    ProptestConfig {
        cases: 8,
        max_shrink_iters: max_shrink,
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            // proptest expects a 'static str, so leak the resolved path.
            Box::leak(regressions.to_string_lossy().into_owned().into_boxed_str()),
        ))),
        ..ProptestConfig::default()
    }
}

proptest_state_machine::prop_state_machine! {
    #![proptest_config(pbt_config())]

    #[test]
    fn general_e2e_pbt(sequential 3..20 => E2ESut<Full>);
}

proptest_state_machine::prop_state_machine! {
    #![proptest_config(pbt_config())]

    #[test]
    fn general_e2e_pbt_sql_only(sequential 3..20 => E2ESut<SqlOnly>);
}

proptest_state_machine::prop_state_machine! {
    #![proptest_config(pbt_config())]

    /// Same as general_e2e_pbt but receives watch_ui events on
    /// futures::executor (not tokio). Catches cross-executor waker bugs
    /// like GPUI blank screen where tokio mpsc wakers don't wake a
    /// non-tokio event loop.
    ///
    /// `#[ignore]` by default: this variant is non-deterministic by
    /// construction — tokio mpsc wakers don't reliably wake the
    /// `futures::executor::block_on` thread, so the same seed can
    /// pass or fail depending on scheduler timing. That's exactly
    /// the bug class it exists to surface, but it also means it is
    /// unsuitable for the default regression sweep (where we want
    /// "fixed seed → fixed outcome"). Run explicitly when you need
    /// to investigate a cross-executor waker issue:
    ///
    ///   cargo nextest run -p holon-integration-tests \
    ///       general_e2e_pbt_cross_executor --run-ignored only
    #[test]
    #[ignore = "non-deterministic by design; run with --ignored to opt in"]
    fn general_e2e_pbt_cross_executor(sequential 3..20 => E2ESut<CrossExecutor>);
}
