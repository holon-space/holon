//! `inv-overlay-placement-local` — where the receiver hangs a shared page is
//! the receiver's own data: moving it moves it on the receiver and nowhere
//! else.
//!
//! @pbt oracle model — the page shares' predicted parents vs. both peers'
//!   `block_raw`.
//! @pbt covers two-instance-page-share — a recipient move that writes the
//!   shared doc (the owner's page moves too) or that never reaches the
//!   receiver's projection; a row positioned by anything but its mount; a
//!   recipient delete that deletes the owner's page or leaves rows behind.

use holon_pbt_core::capabilities::RefSharedView;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutReceiverBackend;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvOverlayPlacementLocal;

impl InvOverlayPlacementLocal {
    pub const ID: InvariantId = InvariantId("inv-overlay-placement-local");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvOverlayPlacementLocal
where
    R: RefSharedView,
    S: SutReceiverBackend + SutBackend,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let shares = ref_.page_shares();
        if shares.is_empty() {
            return InvariantResult::Skipped("the model records no per-page share".into());
        }
        let owner = sut.block_raw_snapshot().await;
        let receiver = sut.receiver_block_raw_snapshot().await;
        for (page, share) in shares {
            let Some(on_owner) = owner.iter().find(|b| b.id == page) else {
                return InvariantResult::Fail(format!(
                    "[inv-overlay-placement-local] the owner holds no row for its shared page \
                     {page} (receiver moved it: {}, receiver left it: {})",
                    share.moved, share.left
                ));
            };
            if on_owner.parent_id != share.owner_parent {
                return InvariantResult::Fail(format!(
                    "[inv-overlay-placement-local] the receiver's placement of {page} reached the \
                     owner: the owner's page now sits under {}, not {}",
                    on_owner.parent_id, share.owner_parent
                ));
            }

            if share.left {
                if sut.receiver_authority_parent(&page).await.is_some() {
                    return InvariantResult::Fail(format!(
                        "[inv-overlay-placement-local] the receiver deleted the shared page \
                         {page}, yet its Loro authority still holds it — the delete did not \
                         leave the share"
                    ));
                }
                let subtree = ref_.page_share_subtree(&page);
                if let Some(kept) = receiver.iter().find(|b| subtree.contains(&b.id)) {
                    return InvariantResult::Fail(format!(
                        "[inv-overlay-placement-local] the receiver left the share of {page}, yet \
                         still holds its row {} {:?}",
                        kept.id, kept.content
                    ));
                }
                continue;
            }

            let Some(on_receiver) = receiver.iter().find(|b| b.id == page) else {
                return InvariantResult::Fail(format!(
                    "[inv-overlay-placement-local] the receiver holds the share of {page} and no \
                     row for it"
                ));
            };
            if on_receiver.parent_id != share.receiver_parent {
                return InvariantResult::Fail(format!(
                    "[inv-overlay-placement-local] the receiver placed the shared page {page} \
                     under {}, but its row sits under {}",
                    share.receiver_parent, on_receiver.parent_id
                ));
            }
            let authority_parent = sut.receiver_authority_parent(&page).await;
            if authority_parent.as_ref() != Some(&share.receiver_parent) {
                return InvariantResult::Fail(format!(
                    "[inv-overlay-placement-local] the receiver's row {page} sits under {}, but \
                     its Loro authority places the page under {authority_parent:?}",
                    on_receiver.parent_id
                ));
            }
            // The position is the placement's too: the row's sort key is the
            // mount's, never the key the page has in the shared doc.
            let row_key = sut.receiver_row_sort_key(&page).await;
            let placement_key = sut.receiver_placement_sort_key(&page).await;
            if row_key.is_none() || row_key != placement_key {
                return InvariantResult::Fail(format!(
                    "[inv-overlay-placement-local] the receiver's row {page} has sort key \
                     {row_key:?}, but the mount that places it sits at {placement_key:?}"
                ));
            }
        }
        InvariantResult::Ok
    }
}
