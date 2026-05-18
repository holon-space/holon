//! Phase 7 — `inv-frontend-bounds-rendered` (DEFERRED).
//!
//! Inline body lives at `sut.rs:6066–6548` (the large BoundsRegistry sub-check
//! block inside `inv-frontend-engine`).
//!
//! # Why deferred
//!
//! The body is a compound of ~10 sub-invariants (bounds-registry-not-empty,
//! expected-size-satisfied, no-error-widgets-rendered, known-widget-type,
//! element-has-content, vm-y-order-and-contiguity, non-wrapper-content-when-docs,
//! not-visually-empty, vm-data-tracked-as-content). Each sub-check reads:
//! - `geometry.all_elements()` — private `frontend_geometry` field returning
//!   `HashMap<String, ElementInfo>` (not exposed via `SutLayout`)
//! - `geometry.element_info(el_id)` — lookup by element id
//! - `info.expected_size`, `info.widget_type`, `info.has_content`, `info.entity_id`,
//!   `info.x`, `info.y` — per-element fields of `ElementInfo`
//! - `ref_state.documents`, `ref_state.focused_entity_id` — no Ref* cap today
//! - `self.frontend_visual_state` — private screenshot analysis field
//!
//! `SutLayout` exposes only `has_registered_bounds`, `has_draggable_handle`,
//! `any_error_widget` — far too thin for these sub-checks. Wire when `SutLayout`
//! grows `all_elements()` and the per-element field surface.

pub struct InvFrontendBoundsRendered;

impl InvFrontendBoundsRendered {
    pub const ID: holon_pbt_core::invariant::InvariantId =
        holon_pbt_core::invariant::InvariantId("inv-frontend-bounds-rendered");
}

// `Invariant` impl intentionally omitted — body migration deferred.
