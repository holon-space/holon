//! `inv-two-instance-convergence` wired into the composed catalog.
//!
//! `Needs SutReceiverBackend + SutTwoInstance + RefSharedView`. Only the
//! two-instance slice supplies the SUT caps, so this deselects (disclosed) on
//! every single-instance draw — including the keystone — and cannot false-RED
//! it.

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

use crate::pbt::invariants::bodies::two_instance_convergence::InvTwoInstanceConvergence;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvTwoInstanceConvergence,
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
