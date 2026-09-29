//! `inv-shape-sim-matches-authority` wired into the composed slice — needs
//! `SutShapeEdit`, whose component reads the production shape audit.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutShapeEdit;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::shape_sim_matches_authority::InvShapeSimMatchesAuthority;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvShapeSimMatchesAuthority,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutShapeEdit>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
        Attribution::at(Layer::OrgRoundTrip, file!()),
    ))
}
