//! `inv-blocks-match-ref/org` — the org store of the block-equivalence
//! composite.
//!
//! Its former sibling stores — `/block_raw`, `/matview`, `/loro` — are now
//! correspondence-registry entries
//! ([`crate::pbt::composed::correspondences::non_seed_blocks`]); this body
//! stays hand-written until Phase 3 because it checks a distinct observable
//! facet: field equality AND per-parent sibling ORDER (disk = the renderer's
//! canonical order), via `compare_blocks(check_order=true)`.
//!
//! `inv-live-children-match-ref` stays its own body: it checks SQL `sort_key`
//! order, not the org store's renderer-canonical sequence order.

use holon_pbt_core::capabilities::{RefBackend, SutOrgRead};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

use crate::pbt::invariants::block_compare::compare_blocks;

/// `/org` store — the blocks parsed back off the on-disk org files
/// ([`SutOrgRead::org_block_snapshot`]) match the reference's org view
/// ([`RefBackend::org_blocks`] — non-seed, non-page, with the org parser's
/// `file:<filename>` parent for unresolved docs). Runs BOTH facets: field
/// equality AND per-parent sibling ORDER (`compare_blocks(check_order=true)`),
/// subsuming a field-equality + per-parent order check. Strict. This is the
/// only block store whose order is checked here: disk order is the renderer's
/// canonical order, whereas
/// `inv-live-children-match-ref` checks SQL `sort_key` order separately.
pub struct InvBlocksMatchRefOrg;

impl InvBlocksMatchRefOrg {
    pub const ID: InvariantId = InvariantId("inv-blocks-match-ref/org");
    const LABEL: &'static str = "inv-blocks-match-ref/org";
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvBlocksMatchRefOrg
where
    R: RefBackend,
    S: SutOrgRead,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let org_blocks = sut.org_block_snapshot().await;
        let ref_blocks_org = ref_.org_blocks();
        compare_blocks(Self::LABEL, &org_blocks, &ref_blocks_org, true)
    }
}
