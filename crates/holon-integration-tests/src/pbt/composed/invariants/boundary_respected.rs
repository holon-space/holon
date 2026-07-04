//! `inv-boundary-respected` wired into the composed catalog.
//!
//! Same `Needs` as `inv-two-instance-convergence` (the two halves of the same
//! cross-instance question: what MUST arrive, and what must NOT), so both
//! select together on the two-instance slice and deselect together elsewhere.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefSharedView;
use holon_pbt_core::capabilities::SutReceiverBackend;
use holon_pbt_core::capabilities::SutTwoInstance;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::boundary_respected::InvBoundaryRespected;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvBoundaryRespected,
        RunMode::Strict,
        Needs {
            sut_present: vec![
                CapId::of::<dyn SutReceiverBackend>(),
                CapId::of::<dyn SutTwoInstance>(),
            ],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefSharedView>()],
        },
        Attribution::at(Layer::StoreCrdt, file!()),
    ))
}
