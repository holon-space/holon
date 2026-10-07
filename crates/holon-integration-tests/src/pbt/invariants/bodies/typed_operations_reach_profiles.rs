//! `inv-typed-operations-reach-profiles` — every free-standing type declared at
//! runtime offers the same operations on a rendered row as the dispatcher
//! accepts for it.
//!
//! @pbt oracle correspondence
//! @pbt covers typed-operations-reach-profiles — for each declared type, the
//!   profile resolver's operation names, and those a row re-resolved on the
//!   profile signal carries, equal the dispatcher catalog's, and the catalog
//!   carries the CRUD triple the declaration registered.
//! @pbt slips-if-removed a write authority registered after boot would be
//!   reachable through MCP and dispatch while no rendered row of its entity
//!   offered a single operation, or while an open view never re-rendered to
//!   offer them.

use std::collections::BTreeSet;

use holon_pbt_core::capabilities::RefTypedEntities;
use holon_pbt_core::capabilities::SutTypedEntity;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

use super::operation_surfaces_agree::operation_surfaces_agree;

pub struct InvTypedOperationsReachProfiles;

impl InvTypedOperationsReachProfiles {
    pub const ID: InvariantId = InvariantId("inv-typed-operations-reach-profiles");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvTypedOperationsReachProfiles
where
    R: RefTypedEntities,
    S: SutTypedEntity,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let crud: BTreeSet<String> = ["create", "set_field", "delete"]
            .into_iter()
            .map(String::from)
            .collect();
        for (type_name, _) in ref_.typed_entity_schemas() {
            if let Err(surfaces) = operation_surfaces_agree(&crud, async || {
                sut.typed_entity_operation_surfaces(&type_name).await
            })
            .await
            {
                return InvariantResult::Fail(format!(
                    "[inv-typed-operations-reach-profiles] '{type_name}': a rendered row offers \
                     different operations than the dispatcher accepts\n  dispatcher: {:?}\n  \
                     profile:    {:?}\n  rerendered: {:?}",
                    surfaces.dispatcher, surfaces.profile, surfaces.rerendered
                ));
            }
        }
        InvariantResult::Ok
    }
}
