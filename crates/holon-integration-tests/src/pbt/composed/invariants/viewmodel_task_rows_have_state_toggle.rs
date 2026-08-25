//! `inv-viewmodel-task-rows-have-state-toggle` — every rendered `tree_item`
//! row backed by a ref task block carries a `state_toggle` in its own row
//! scope (the blind-side twin of `inv-viewmodel-state-toggle-correct`, which
//! only judges toggles that exist). `Needs SutRenderer + RefBlockTree +
//! RefTaskState`. The ref side is the production `ReferenceState`; selection
//! ANDs the SUT and ref cap sets, so it only fires where a real renderer
//! slice is wired (the frontend slice).

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefTaskState;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::task_rows_have_state_toggle::InvViewmodelTaskRowsHaveStateToggle;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvViewmodelTaskRowsHaveStateToggle,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutRenderer>()],
            sut_absent: Vec::new(),
            ref_present: vec![
                CapId::of::<dyn RefBlockTree>(),
                CapId::of::<dyn RefTaskState>(),
            ],
        },
        Attribution::at(Layer::ViewModel, file!()),
    ))
}
