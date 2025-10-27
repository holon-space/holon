//! `inv-viewmodel-editable-text-triggers` — every editable-text widget in the
//! rendered tree carries its expected trigger wiring. `Needs SutRenderer` only
//! (no reference): a SUT-internal contract on the rendered tree. Selected by
//! any slice with a renderer — the frontend slice's real headless
//! `ReactiveEngine`. This is the last `SutRenderer` native consumer; porting it
//! completes `SutRenderer`'s composed coverage.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::viewmodel_editable_text_triggers::InvViewmodelEditableTextTriggers;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvViewmodelEditableTextTriggers,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutRenderer>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
    ))
}
