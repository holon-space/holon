//! `inv-share-mount-carries-page-identity` wired into the composed catalog.
//!
//! `Needs SutReceiverBackend + SutBackend + RefSharedView`. Only the
//! two-instance slice supplies `SutReceiverBackend`, so this deselects
//! (disclosed) on every single-instance draw.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefSharedView;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutReceiverBackend;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::share_mount_carries_page_identity::InvShareMountCarriesPageIdentity;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvShareMountCarriesPageIdentity,
        RunMode::Strict,
        Needs {
            sut_present: vec![
                CapId::of::<dyn SutReceiverBackend>(),
                CapId::of::<dyn SutBackend>(),
            ],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefSharedView>()],
        },
        Attribution::at(Layer::StoreCrdt, file!()),
    ))
}
