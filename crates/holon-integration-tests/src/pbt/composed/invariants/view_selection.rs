//! `inv-view-selection` — the SUT's current view ([`SutViewModel::current_view`])
//! matches the reference's expected view ([`RefRender::current_view`]). `Needs
//! SutViewModel + RefRender`. The ref side is the production `ReferenceState`
//! (which already implements `RefRender`); selection ANDs the SUT and ref cap
//! sets, so it only fires where a real ViewModel slice is wired — today the
//! frontend slice. Teeth run there over the real render pipeline.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefRender, SutViewModel};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::view_selection::InvViewSelection;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvViewSelection,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutViewModel>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefRender>()],
        },
    ))
}
