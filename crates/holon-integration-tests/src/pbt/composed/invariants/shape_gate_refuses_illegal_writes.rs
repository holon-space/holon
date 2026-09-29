//! `inv-shape-gate-refuses-illegal-writes` wired into the composed slice —
//! needs `SutShapeEdit` (the engine the edits went through) and
//! `RefShapeEdits` (the outcomes the model decided).

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefShapeEdits;
use holon_pbt_core::capabilities::SutShapeEdit;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::shape_gate_refuses_illegal_writes::InvShapeGateRefusesIllegalWrites;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvShapeGateRefusesIllegalWrites,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutShapeEdit>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefShapeEdits>()],
        },
        Attribution::at(Layer::OrgRoundTrip, file!()),
    ))
}
