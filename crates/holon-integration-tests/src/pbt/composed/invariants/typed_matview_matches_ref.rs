//! `inv-typed-matview-matches-ref` wired into the composed catalog — every
//! free-standing type's read matview matches the datatype-axis oracle and no
//! typed-entity id leaks into a block table (BG-1).
//!
//! `Needs SutTypedEntity` (SUT) + `RefTypedEntities` (ref). Only the
//! Turso+frontend arm supplies `SutTypedEntity`, so a Loro-only / storage-only
//! slice deselects honestly rather than running against an absent
//! serialization.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefTypedEntities;
use holon_pbt_core::capabilities::SutTypedEntity;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::typed_matview_matches_ref::InvTypedMatviewMatchesRef;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvTypedMatviewMatchesRef,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutTypedEntity>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefTypedEntities>()],
        },
        Attribution::at(Layer::Projection, file!()),
    ))
}
