//! Phase 7 — `inv-matview-consistent-with-ref` (DEFERRED).
//!
//! Inline body lives at `sut.rs:5510–5612` (inside the ReactiveEngine snapshot
//! block, labelled inv-matview-consistent-with-ref).
//!
//! # Why deferred
//!
//! The body requires:
//! - `reactive.ensure_watching(&root_id)` snapshot `data_rows` — these come from
//!   the private `self.reactive_engine` field via `reactive.ensure_watching` +
//!   `results.snapshot()`. There is no `SutSqlProjection` method that returns the
//!   ReactiveEngine's data_rows for the root layout.
//! - `ref_state.block_state.blocks`, `ref_state.layout_blocks`, `ref_state.profile_block_ids`,
//!   `ref_state.is_descendant_of_any`, `ref_state.expected_focus_root_ids` — no Ref*
//!   cap today covers these fields.
//! - The "soft check" semantics (eprintln not panic) are a deliberate design choice
//!   that would need `RunMode::Warn` plus a `Skipped`-path classifier.
//!
//! Wire when: the reactive-engine data_rows surface is exposed via a new
//! `SutViewModel::root_layout_data_row_ids()` method and Ref* caps cover the
//! block-tree + layout-block + focus-root query surface.

pub struct InvMatviewConsistentWithRef;

impl InvMatviewConsistentWithRef {
    pub const ID: holon_pbt_core::invariant::InvariantId =
        holon_pbt_core::invariant::InvariantId("inv-matview-consistent-with-ref");
}

// Status: ref-side unblocked (RefFocusRoots, RefLayout added).
// Remaining blocker: data_rows come from self.reactive_engine (private RefCell)
// via reactive.ensure_watching; no cap exposes this surface yet.
// Promote when SutViewModel::root_layout_data_row_ids is added and wired.
