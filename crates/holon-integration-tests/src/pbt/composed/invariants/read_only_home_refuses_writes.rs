//! `inv-read-only-home-refuses-writes` wired into the composed slice — needs
//! `SutReadOnlyHomes` (the production write-tier authority's view of the vault)
//! and `RefReadOnlyHomes` (which blocks the fixture homed read-only).

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefReadOnlyHomes;
use holon_pbt_core::capabilities::SutReadOnlyHomes;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::read_only_home_refuses_writes::InvReadOnlyHomeRefusesWrites;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvReadOnlyHomeRefusesWrites,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutReadOnlyHomes>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefReadOnlyHomes>()],
        },
        Attribution::at(Layer::OrgRoundTrip, file!()),
    ))
}
