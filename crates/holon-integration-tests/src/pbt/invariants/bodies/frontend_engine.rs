//! Phase 7 — `inv-frontend-engine` (DEFERRED).
//!
//! Inline body lives at `sut.rs:6023–6547`.
//!
//! # Why deferred
//!
//! The body orchestrates a large block: it reads `self.frontend_engine` (pub),
//! calls `fe_engine.ensure_watching`, `fe_engine.snapshot`, then delegates to
//! the BoundsRegistry sub-checks and the `inv-frontend-bounds-rendered` compound.
//! Key blockers:
//! - `self.frontend_engine` is accessed directly by field (pub) but the reactive
//!   engine watch+snapshot lifecycle needs to be exposed via `SutViewModel` or
//!   a new `SutFrontendEngine` trait to be composable with `Invariant<R, S>`
//! - `geometry.all_elements()` (ElementInfo map) not in `SutLayout`
//! - `ref_state.documents` / `ref_state.focused_entity_id` not in any Ref* cap
//!
//! Wire as part of the FrontendBounds cluster migration (same phase as
//! `inv-frontend-bounds-rendered`).

pub struct InvFrontendEngine;

impl InvFrontendEngine {
    pub const ID: holon_pbt_core::invariant::InvariantId =
        holon_pbt_core::invariant::InvariantId("inv-frontend-engine");
}

// `Invariant` impl intentionally omitted — body migration deferred.
