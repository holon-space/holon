//! `inv-integration-op-routable-or-refused` wired into the composed catalog.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefIntegrationConnect;
use holon_pbt_core::capabilities::SutIntegrationConnect;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::integration_op_routable_or_refused::InvIntegrationOpRoutableOrRefused;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvIntegrationOpRoutableOrRefused,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutIntegrationConnect>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefIntegrationConnect>()],
        },
        Attribution::at(Layer::Projection, file!()),
    ))
}
