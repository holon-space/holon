//! `inv-viewmodel-state-toggle-correct` — the rendered StateToggle nodes
//! reflect the reference's block task_state. `Needs SutRenderer + RefBlockTree
//! + RefTaskState`. The ref side is the production `ReferenceState`; selection
//! ANDs the SUT and ref cap sets, so it only fires where a real renderer slice
//! is wired (the frontend slice).

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefTaskState;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::viewmodel_state_toggle_correct::InvViewmodelStateToggleCorrect;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvViewmodelStateToggleCorrect,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutRenderer>()],
            sut_absent: Vec::new(),
            ref_present: vec![
                CapId::of::<dyn RefBlockTree>(),
                CapId::of::<dyn RefTaskState>(),
            ],
        },
    ))
}
