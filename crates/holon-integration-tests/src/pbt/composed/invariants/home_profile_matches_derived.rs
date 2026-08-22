//! `inv-home-profile-matches-derived` wired into the composed catalog — the
//! home-profile binding of 2b.4 (CV-D stage A).
//!
//! `Needs SutHomeProfile` (production's resolved home → profile id) +
//! `RefDocuments` (the draw's own document bookkeeping). The reference NEVER
//! calls `profile_for`; see the body's doc comment for why that rule is what
//! makes this invariant able to fail at all.
//!
//! Vacuously green on a draw with no blocks, so it adds no false RED to slices
//! that seed nothing.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefDocuments;
use holon_pbt_core::capabilities::SutHomeProfile;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::home_profile_matches_derived::InvHomeProfileMatchesDerived;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvHomeProfileMatchesDerived,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutHomeProfile>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefDocuments>()],
        },
        Attribution::at(Layer::OrgRoundTrip, file!()),
    ))
}
