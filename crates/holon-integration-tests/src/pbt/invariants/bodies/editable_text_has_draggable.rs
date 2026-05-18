//! Phase 7 — `inv-editable-text-has-draggable` (DEFERRED).
//!
//! Inline body lives at `sut.rs:6549–6707`.
//!
//! # Why deferred
//!
//! The body requires:
//! - `self.frontend_engine` / `self.reactive_engine` (private) — used to
//!   walk the full resolved ViewModel BFS across `live_block` children via
//!   `engine.snapshot_reactive(&uri)` and `holon_frontend::focus_path::walk_tree`
//! - `self.reactive_root_id` (private) — to determine the BFS root
//! - `ref_state.has_blocks_profile()` — ref-side gate that decides whether
//!   the test profile's render overrides block_profile (no Ref* cap trait today)
//!
//! The capability surface would need `SutRenderer::walk_draggable_pairs()` or
//! similar returning `(editable_ids, draggable_ids)` per tree — a non-trivial
//! BFS across the reactive engine. Wire alongside `inv-editable-text-has-draggable`
//! proper in the phase that migrates FrontendBounds + Renderer cluster invariants.

pub struct InvEditableTextHasDraggable;

impl InvEditableTextHasDraggable {
    pub const ID: holon_pbt_core::invariant::InvariantId =
        holon_pbt_core::invariant::InvariantId("inv-editable-text-has-draggable");
}

// Status: ref-side unblocked (RefLayout::has_blocks_profile added).
// Remaining blocker: body needs frontend_engine / reactive_engine (private)
// plus reactive_root_id (private) for the BFS root. No cap exposes the
// walk_draggable_pairs surface yet. Promote in the FrontendBounds/Renderer pass.
