//! `inv-overlay-placement-local` — where the receiver hangs a shared page is
//! the receiver's own data: moving it moves it on the receiver and nowhere
//! else.
//!
//! @pbt oracle model — the page shares' predicted parents vs. both peers'
//!   `block_raw`.
//! @pbt covers two-instance-page-share — a recipient move that writes the
//!   shared doc (the owner's page moves too) or that never reaches the
//!   receiver's projection.

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
        let moved: Vec<_> = ref_
            .page_shares()
            .into_iter()
            .filter(|(_, share)| share.moved)
            .collect();
        if moved.is_empty() {
            return InvariantResult::Skipped(
                "the model records no receiver move of a shared page".into(),
            );
        }
        let owner = sut.block_raw_snapshot().await;
        let receiver = sut.receiver_block_raw_snapshot().await;
        for (page, share) in moved {
            let Some(on_receiver) = receiver.iter().find(|b| b.id == page) else {
                return InvariantResult::Fail(format!(
                    "[inv-overlay-placement-local] the receiver moved the shared page {page} and \
                     now holds no row for it"
                ));
            };
            if on_receiver.parent_id != share.receiver_parent {
                return InvariantResult::Fail(format!(
                    "[inv-overlay-placement-local] the receiver moved the shared page {page} \
                     under {}, but its row sits under {}",
                    share.receiver_parent, on_receiver.parent_id
                ));
            }
            let Some(on_owner) = owner.iter().find(|b| b.id == page) else {
                return InvariantResult::Fail(format!(
                    "[inv-overlay-placement-local] the owner holds no row for its shared page \
                     {page} after the receiver moved it"
                ));
            };
            if on_owner.parent_id != share.owner_parent {
                return InvariantResult::Fail(format!(
                    "[inv-overlay-placement-local] the receiver's move of {page} reached the \
                     owner: the owner's page now sits under {}, not {}",
                    on_owner.parent_id, share.owner_parent
                ));
            }
        }
        InvariantResult::Ok
    }
}
