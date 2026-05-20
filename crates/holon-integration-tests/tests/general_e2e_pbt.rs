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
//!       transition. Tests file watcher's multi-event processing and FileSyncController's
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
//
// # Localising a failure across layers
//
// On a strict failure the runner already panics with a cross-layer report —
// every invariant marked ✓/✗/⊘/⚠, grouped by the subsystem each touches, with
// a "trouble begins at: <layer>" headline naming the lowest diverging layer
// (so layers below it are exonerated). To get that table *without* a strict
// panic — to localise a Warn-level divergence (e.g. `inv-displayed-text/
// viewmodel`) or just inspect which layers are healthy — replay the persisted
// seed with `HOLON_PBT_LAYER_REPORT`:
//
//   HOLON_PBT_LAYER_REPORT=warn PROPTEST_CASES=1 PROPTEST_MAX_SHRINK_ITERS=0 \
//     RUST_LOG=pbt_invariant=warn \
//     cargo test -p holon-integration-tests --test general_e2e_pbt \
//     general_e2e_pbt_sql_only -- --nocapture --exact
//
// `=warn` emits the report (via `tracing`, no panic) on every tick that has a
// Warn or Fail; `=always` emits on every tick regardless (verbose). This
// replaces the old `HOLON_PBT_INVARIANTS=*:warn` + grep recipe — it keeps
// strict semantics intact and just adds the readable table.
//
// # Bug-class reproducers
//
// The atomic editor primitives (`TypeChars` → `PressKey(Enter)`) reproduce
// the split_block-discards-pending-edits bug in ~25s with biased weights:
//
//   PROPTEST_CASES=1 PROPTEST_MAX_SHRINK_ITERS=0 \
//     HOLON_PBT_WEIGHTS="ClickBlock:30,FocusEditableText:50,TypeChars:50,PressKey:50,Navigate*:0" \
//     cargo test -p holon-integration-tests --test general_e2e_pbt \
//     general_e2e_pbt_sql_only -- --nocapture --exact
//
// `ClickBlock:30` is needed so focus moves from `block:journals` (default)
// to a user doc where `BulkExternalAdd` placed text-content blocks; without
// it `FocusEditableText` finds no candidates and never fires.
// `Navigate*:0` silences a separate (pre-existing) NavigateBack focus
// mismatch that masks this bug otherwise. Devlog:
// `devlog/2026-05-08-154449-split-block-discards-pending-edits.md`.

use holon_integration_tests::component_pbt;
use holon_integration_tests::pbt::standard_pbt_config;
use holon_pbt_core::ComponentSet;

// Both variants exercise the same state machine and so share one
// `*.proptest-regressions` file — they pass the same `slice_name` to
// `standard_pbt_config` (which also activates the atomic editor primitives and
// installs the rejection-histogram panic hook). `cases` defaults to 8;
// `PROPTEST_CASES` / `PROPTEST_MAX_SHRINK_ITERS` override at runtime.
component_pbt! {
    test_fn: general_e2e_pbt,
    set: ComponentSet::full_headless(),
    proptest_config: standard_pbt_config("general_e2e_pbt"),
    steps: 3..20,
}

component_pbt! {
    test_fn: general_e2e_pbt_sql_only,
    set: ComponentSet::sql_only(),
    proptest_config: standard_pbt_config("general_e2e_pbt"),
    steps: 3..20,
}
