//! `inv-typed-loro-matches-ref` wired into the composed catalog.
//!
//! `Needs SutTypedEntityLoro` (SUT) + `RefTypedEntities` (ref). Every Loro arm
//! supplies the Loro read, so a SqlOnly draw deselects honestly.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefTypedEntities;
use holon_pbt_core::capabilities::SutTypedEntityLoro;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::typed_loro_matches_ref::InvTypedLoroMatchesRef;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvTypedLoroMatchesRef,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutTypedEntityLoro>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefTypedEntities>()],
        },
        Attribution::at(Layer::Projection, file!()),
    ))
}
