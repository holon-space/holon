//! Phase 7 — `inv-viewmodel-tree-virtual-slots` (DEFERRED).
//!
//! Inline body lives at `sut.rs:5447–5508` (inside ReactiveEngine snapshot
//! block, sub-check 10j).
//!
//! # Why deferred
//!
//! The body is explicitly marked as `SKIPPED` in sut.rs itself ("display_tree must
//! be obtained from wait_for_entity_in_resolved_view_model or similar before this
//! invariant can meaningfully execute"). It requires:
//! - `display_tree` from private reactive_engine path (same blocker as inv-viewmodel-snapshot)
//! - `ref_state.active_render_expr_name(Region::Main)` — no Ref* cap
//! - `holon_frontend::ReactiveViewModel` collection walk internals
//!
//! The inline code sets `found = 0; without = 0; let _ = (found, without);` and then
//! immediately emits the SKIPPED log — effectively a no-op. Wire when the
//! `display_tree` blocker (inv-viewmodel-snapshot) is resolved AND when the walk
//! infra is exposed via a cap method.

pub struct InvViewmodelTreeVirtualSlots;

impl InvViewmodelTreeVirtualSlots {
    pub const ID: holon_pbt_core::invariant::InvariantId =
        holon_pbt_core::invariant::InvariantId("inv-viewmodel-tree-virtual-slots");
}

// `Invariant` impl intentionally omitted — body migration deferred.
