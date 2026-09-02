//! `inv-two-writer-peer-writes-land` wired into the composed catalog.
//!
//! `Needs SutReceiverBackend + SutTwoInstance + SutSqlProjection + SutBackend +
//! RefSharedView`. Only the two-instance slice supplies the two-instance SUT
//! caps, so this deselects (disclosed) on every single-instance draw.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefSharedView;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutReceiverBackend;
use holon_pbt_core::capabilities::SutSqlProjection;
use holon_pbt_core::capabilities::SutTwoInstance;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::two_writer_peer_writes_land::InvTwoWriterPeerWritesLand;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvTwoWriterPeerWritesLand,
        RunMode::Strict,
        Needs {
            sut_present: vec![
                CapId::of::<dyn SutReceiverBackend>(),
                CapId::of::<dyn SutTwoInstance>(),
                CapId::of::<dyn SutSqlProjection>(),
                CapId::of::<dyn SutBackend>(),
            ],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefSharedView>()],
        },
        Attribution::at(Layer::StoreCrdt, file!()),
    ))
}
