//! `inv-frontend-no-error-widgets` wired into the composed catalog — the
//! **windowed** sibling of `inv-viewmodel-no-error-widgets`. `Needs SutViewModel +
//! SutLayout` (the laid-out widget tree **and** the ViewModel it renders from, one
//! pipeline); no ref. Distinct from the viewmodel-only variant: `SutLayout::
//! any_error_widget` also catches `Error` widgets in the live `BoundsRegistry`
//! (the geometry layer), not just the ViewModel tree.
//!
//! Selected only by the windowed slice (`window_slice::window_focus_wide`): the
//! headless `frontend_slice` has a `SutViewModel` but no `SutLayout`, so it is
//! deselected — disclosed, not faked. Teeth come from the real
//! `run_windowed_composed_check` every tick (no fixture triad — see the
//! `pbt-composition` skill, testing philosophy); `cargo-mutants` is the deferred
//! detection gate.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{SutLayout, SutViewModel};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::frontend_no_error_widgets::InvFrontendNoErrorWidgets;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvFrontendNoErrorWidgets,
        RunMode::Strict,
        Needs {
            sut_present: vec![
                CapId::of::<dyn SutViewModel>(),
                CapId::of::<dyn SutLayout>(),
            ],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
    ))
}
