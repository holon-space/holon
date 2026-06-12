//! `inv-frontend-bounds-rendered` wired into the composed catalog — the first
//! **windowed** registry invariant. `Needs SutLayout + SutViewModel` (real
//! geometry **and** the ViewModel it is compared against, from one render
//! pipeline) and `RefLayout` (document / focus metadata gating the content
//! checks). Selected only by the windowed slice (`window_slice::window_wide`):
//! the headless `frontend_slice` has a `SutViewModel` but no `SutLayout`, so it
//! is deselected — disclosed, not faked.
//!
//! `Strict`. The body internally `Skip`s when the frontend root is still loading
//! (no `frontend_root_vm`), and downgrades its document-gated content checks when
//! the reference has no user documents — so over a minimal oracle it runs its
//! strict geometry checks (expected-size, no-error-widgets, VM y-order /
//! contiguity) over the window's real bounds.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefLayout, SutLayout, SutViewModel};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::frontend_bounds_rendered::InvFrontendBoundsRendered;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvFrontendBoundsRendered,
        RunMode::Strict,
        Needs {
            sut_present: vec![
                CapId::of::<dyn SutLayout>(),
                CapId::of::<dyn SutViewModel>(),
            ],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefLayout>()],
        },
    ))
}
