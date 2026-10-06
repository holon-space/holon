//! `inv-typed-operations-reach-profiles` wired into the composed catalog — a
//! type declared at runtime offers on its rendered rows the operations the
//! dispatcher accepts for it.
//!
//! `Needs SutTypedEntity` (SUT) + `RefTypedEntities` (ref), the same arm as
//! `inv-typed-matview-matches-ref`.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefTypedEntities;
use holon_pbt_core::capabilities::SutTypedEntity;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::typed_operations_reach_profiles::InvTypedOperationsReachProfiles;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvTypedOperationsReachProfiles,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutTypedEntity>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefTypedEntities>()],
        },
        Attribution::at(Layer::Projection, file!()),
    ))
}
