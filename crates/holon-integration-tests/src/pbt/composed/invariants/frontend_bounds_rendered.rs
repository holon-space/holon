//! `inv-frontend-bounds-rendered` wired into the composed catalog — the first
//! **windowed** registry invariant. `Needs SutLayout + SutFrontendEngine` (real
//! geometry **and** the root ViewModel it is compared against, from one render
//! pipeline) and `RefLayout` (document / focus metadata gating the content
//! checks). Selected only by the windowed slice: `SutFrontendEngine` (the live
//! gpui root-VM resolution surface, C-5 Tier-2) is registered only where a live
//! window's engine is present, and the headless `frontend_slice` has neither it
//! nor `SutLayout`, so it is deselected — disclosed, not faked.
//!
//! `Strict`. The body internally `Skip`s when the frontend root is still loading
//! (no `frontend_root_vm`), and downgrades its document-gated content checks when
//! the reference has no user documents — so over a minimal oracle it runs its
//! strict geometry checks (expected-size, no-error-widgets, VM y-order /
//! contiguity) over the window's real bounds.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefLayout, SutFrontendEngine, SutLayout};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::frontend_bounds_rendered::InvFrontendBoundsRendered;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvFrontendBoundsRendered,
        RunMode::Strict,
        Needs {
            sut_present: vec![
                CapId::of::<dyn SutLayout>(),
                CapId::of::<dyn SutFrontendEngine>(),
            ],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefLayout>()],
        },
    ))
}
