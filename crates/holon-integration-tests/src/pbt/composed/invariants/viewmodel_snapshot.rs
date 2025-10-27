//! `inv-viewmodel-snapshot` — the headless `widget_tree_snapshot()` is
//! internally consistent. `Needs SutRenderer` only (no reference): a
//! SUT-internal property of the render pipeline. Selected by any slice with a
//! renderer — the frontend slice's real headless `ReactiveEngine`, where the
//! snapshot is meaningful.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::viewmodel_snapshot::InvViewmodelSnapshot;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvViewmodelSnapshot,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutRenderer>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
    ))
}
