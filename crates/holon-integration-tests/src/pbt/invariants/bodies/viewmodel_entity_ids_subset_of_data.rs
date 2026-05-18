//! Phase 7 — `inv-viewmodel-entity-ids-subset-of-data` (DEFERRED).
//!
//! Inline body lives at `sut.rs:5064–5113` (inside the ReactiveEngine snapshot
//! block).
//!
//! # Why deferred
//!
//! The body requires:
//! - `display_tree.collect_entity_ids()` — produced from private `self.reactive_engine`
//! - `data_rows` snapshot from `reactive.ensure_watching` — private engine
//! - `ref_state.root_render_expr().is_some()` — ref gate, no Ref* cap
//!
//! Wire when `SutViewModel` exposes `tree_entity_ids()` + `data_row_ids()`
//! and `RefBlockTree` covers `root_render_expr`.

pub struct InvViewmodelEntityIdsSubsetOfData;

impl InvViewmodelEntityIdsSubsetOfData {
    pub const ID: holon_pbt_core::invariant::InvariantId =
        holon_pbt_core::invariant::InvariantId("inv-viewmodel-entity-ids-subset-of-data");
}

// Status: ref-side unblocked (RefRender::has_root_render_expr added).
// Remaining blocker: display_tree and data_rows both come from private
// reactive_engine RefCell; no cap exposes this surface yet.
// Promote when SutViewModel::tree_entity_ids + data_row_ids are wired.
