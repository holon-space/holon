//! `inv-typed-matview-matches-ref` — every free-standing type's read matview
//! (the `TursoAdapter` derivation) matches the datatype-axis oracle, and no
//! typed-entity id ever appears in a block table.
//!
//! @pbt oracle correspondence
//! @pbt covers typed-matview-matches-ref — each registered free-standing type's
//!   matview rows vs `RefTypedEntities::expected_typed_entity_rows`, plus the
//!   identity that a free-standing entity's id is absent from `block_raw`.
//! @pbt slips-if-removed a typed entity created via its serialization that
//!   failed to project into the read matview, or whose id collided into a block
//!   table, would go unnoticed — the block invariants never look at the typed
//!   tables.
//!
//! Type-agnostic: the types and their columns come from the oracle, so a newly
//! declared free-standing type is covered without editing this file.
//!
//! `Needs SutTypedEntity` (SUT) + `RefTypedEntities` (ref); only the
//! Turso+frontend arm supplies `SutTypedEntity`, so a Loro-only / storage-only
//! slice deselects honestly.

use std::time::Duration;
use std::time::Instant;

use holon_pbt_core::capabilities::RefTypedEntities;
use holon_pbt_core::capabilities::SutTypedEntity;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvTypedMatviewMatchesRef;

impl InvTypedMatviewMatchesRef {
    pub const ID: InvariantId = InvariantId("inv-typed-matview-matches-ref");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvTypedMatviewMatchesRef
where
    R: RefTypedEntities,
    S: SutTypedEntity,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        for (type_name, columns) in ref_.typed_entity_schemas() {
            let expected = ref_.expected_typed_entity_rows(&type_name);

            // Bounded wait for the IVM matview to converge to the oracle —
            // guards any residual CDC lag past the harness's own convergence
            // gate.
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let actual = sut.typed_entity_rows(&type_name, columns.clone()).await;
                if actual == expected {
                    break;
                }
                if Instant::now() >= deadline {
                    return InvariantResult::Fail(format!(
                        "[inv-typed-matview-matches-ref] '{type_name}' matview != oracle\n  \
                         columns:  {columns:?}\n  \
                         expected: {expected:?}\n  actual:   {actual:?}"
                    ));
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }

        // Datatype-axis identity: a free-standing entity lives in its own table
        // ONLY — no typed-entity id may appear in a block table.
        let block_ids = sut.block_raw_ids().await;
        for id in ref_.typed_entity_ids() {
            if block_ids.contains(&id) {
                return InvariantResult::Fail(format!(
                    "[inv-typed-matview-matches-ref] typed-entity id '{id}' leaked into block_raw"
                ));
            }
        }

        InvariantResult::Ok
    }
}
