//! `inv-viewmodel-tree-virtual-slots` — the rendered ViewModel tree's virtual
//! slots are well-formed. `Needs SutRenderer` only (no reference): a SUT-internal
//! structural property. Selected by any slice with a renderer — the frontend
//! slice's real headless `ReactiveEngine`.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::viewmodel_tree_virtual_slots::InvViewmodelTreeVirtualSlots;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvViewmodelTreeVirtualSlots,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutRenderer>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
    ))
}
