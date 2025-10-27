//! `inv-block-tags-references-exist`.
//!
//! Referential integrity check: every `block_id` in the `block_tags`
//! junction table must have a corresponding row in `block_raw`. Orphan
//! rows are not prevented by a DB-level FK constraint (Turso doesn't
//! enforce FKs in IVM mode), so this invariant catches the gap.
//!
//! Bound: `S: SutSqlProjection` — no ref-side comparison needed. The
//! invariant is entirely self-consistent within the SQL projection: the
//! set of block ids referenced by `block_tags` must be a subset of all
//! block ids in `block_raw`.
//!
//! Bug class caught: any code path that deletes a block without cleaning
//! up its `block_tags` rows (e.g. cascading DELETE not wired, or a
//! junction-row INSERT racing a block DELETE).

use std::collections::BTreeSet;
use std::marker::PhantomData;

use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::SutSqlProjection;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvBlockTagsReferencesExist<R>(pub PhantomData<R>);

impl<R> InvBlockTagsReferencesExist<R> {
    pub const ID: InvariantId = InvariantId("inv-block-tags-references-exist");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvBlockTagsReferencesExist<R>
where
    S: SutSqlProjection,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        let all_ids: BTreeSet<_> = sut.all_block_ids().await;
        let tag_ids: BTreeSet<_> = sut.block_tag_block_ids().await;

        let orphans: Vec<&EntityUri> = tag_ids.difference(&all_ids).collect();

        if orphans.is_empty() {
            return InvariantResult::Ok;
        }

        InvariantResult::Fail(format!(
            "[inv-block-tags-references-exist] {} orphan block_id(s) in block_tags have no \
             corresponding row in block_raw.\n  orphans (up to 10): {:?}\n  \
             block_tags.block_ids.len={}, block_raw.ids.len={}",
            orphans.len(),
            orphans.iter().take(10).collect::<Vec<_>>(),
            tag_ids.len(),
            all_ids.len(),
        ))
    }
}
