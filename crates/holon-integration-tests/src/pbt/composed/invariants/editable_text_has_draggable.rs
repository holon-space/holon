//! `inv-editable-text-has-draggable` — every editable-text widget in the rendered
//! tree has its draggable affordance. `Needs SutRenderer + RefLayout`. The ref side
//! is the production `ReferenceState`; selection ANDs the SUT and ref cap sets, so
//! it only fires where a real renderer slice is wired (the frontend slice).

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefLayout, SutRenderer};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::editable_text_has_draggable::InvEditableTextHasDraggable;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvEditableTextHasDraggable,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutRenderer>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefLayout>()],
        },
    ))
}
