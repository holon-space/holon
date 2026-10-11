//! `inv-typed-loro-matches-ref` — in every Loro mode, the Loro typed-entity
//! store holds exactly the oracle's live free-standing entities.
//!
//! @pbt oracle correspondence
//! @pbt covers typed-loro-matches-ref — each type's live entities read straight
//!   off the Loro map vs `RefTypedEntities::expected_stored_typed_entity_rows`.
//! @pbt slips-if-removed a typed entity whose write never reached Loro (a SQL
//!   writer left in a Loro mode, or no writer at all without Turso) would only
//!   ever live in the ephemeral Turso projection, or nowhere.
//!
//! Compares the stored columns only: computed columns live in the matview.

use holon_pbt_core::capabilities::RefTypedEntities;
use holon_pbt_core::capabilities::SutTypedEntityLoro;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvTypedLoroMatchesRef;

impl InvTypedLoroMatchesRef {
    pub const ID: InvariantId = InvariantId("inv-typed-loro-matches-ref");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvTypedLoroMatchesRef
where
    R: RefTypedEntities,
    S: SutTypedEntityLoro,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        for (type_name, columns) in ref_.stored_typed_entity_schemas() {
            let expected = ref_.expected_stored_typed_entity_rows(&type_name);
            let actual = sut
                .loro_typed_entity_rows(&type_name, columns.clone())
                .await;
            if actual != expected {
                return InvariantResult::Fail(format!(
                    "[inv-typed-loro-matches-ref] '{type_name}' Loro store != oracle\n  \
                     columns:  {columns:?}\n  expected: {expected:?}\n  actual:   {actual:?}"
                ));
            }
        }
        InvariantResult::Ok
    }
}
