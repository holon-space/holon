//! `inv-sidebar-page-tag-preserved` — a block the reference model marks as a
//! `Page` doc-root must STILL carry the `Page` tag in the SUT's live
//! projection.
//!
//! @pbt oracle correspondence — SUT live-projection is_page vs ref
//!   is_page_block, over ids present in the SUT snapshot
//! @pbt covers page-demotion — a Page doc-root whose `Page` tag is stripped in
//!   the SUT projection while its block row survives
//! @pbt slips-if-removed a folder-companion .org inlines a page-file doc-root
//!   as a plain heading, stripping its `Page` tag on cold-boot reconcile; the
//!   page vanishes from the sidebar and the id-set compare can't see it
//!
//! This is the sidebar oracle: the left sidebar renders exactly the blocks
//! whose `tags` contain `"Page"` (`tag='Page'`). A page silently DEMOTED (its
//! `Page` tag stripped) vanishes from the sidebar even though its block row
//! still exists — so a coarse block-id set check (`inv-block-ids-match-ref`)
//! can't see it (the id is still present, and pages are seed-filtered from that
//! compare anyway). This invariant closes that gap by asserting the `Page` TAG
//! survives.
//!
//! The demotion class (dogfood 2026-07-12): a folder-companion `.org` file that
//! inlines another page-file's doc-root as a plain (untagged) heading strips
//! the page-file's `Page` tag on cold-boot reconcile. See
//! `crate::pbt::composed::wide_e2e::seed_folder_companion`.
//!
//! Direction: over every block PRESENT in the SUT projection, if the ref says
//! it is a page then the SUT block must be a page. Restricting to ids present
//! in the SUT snapshot means a page the ref models-but-hasn't-booted (a
//! non-frontend draw) can't false-diverge — it simply isn't in the snapshot to
//! check.
//!
//! `Needs SutBackend` (SUT) + `RefBlockTree` (ref).

use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvSidebarPageTagPreserved;

impl InvSidebarPageTagPreserved {
    pub const ID: InvariantId = InvariantId("inv-sidebar-page-tag-preserved");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvSidebarPageTagPreserved
where
    R: RefBlockTree,
    S: SutBackend,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let demoted: Vec<EntityUri> = sut
            .live_block_snapshot()
            .await
            .into_iter()
            .filter(|b| ref_.is_page_block(&b.id) && !b.is_page())
            .map(|b| b.id)
            .collect();

        if demoted.is_empty() {
            return InvariantResult::Ok;
        }

        InvariantResult::Fail(format!(
            "[inv-sidebar-page-tag-preserved] {} page doc-root(s) DEMOTED — the reference models \
             them as `Page` blocks (sidebar `tag='Page'`) but the SUT projection dropped their \
             `Page` tag, so they vanish from the sidebar: {:?}. Likely a folder-companion `.org` \
             that inlined the page-file's doc-root as a plain heading and stripped its `Page` tag \
             on reconcile.",
            demoted.len(),
            demoted.iter().take(10).collect::<Vec<_>>(),
        ))
    }
}
