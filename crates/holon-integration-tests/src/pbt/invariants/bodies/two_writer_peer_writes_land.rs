//! `inv-two-writer-peer-writes-land` — the SECOND writer's blocks reach the
//! owner, under the parent the model named, and are still the peer's blocks
//! when they get there.
//!
//! @pbt oracle model — the sharing overlay's delivered peer writes vs. both
//!   peers' stores, with CRDT authorship as the provenance witness.
//! @pbt covers two-instance-two-writer — a reverse leg that carries nothing, a
//!   peer block that lands under the wrong parent, and an owner that "has" the
//!   block only because it minted one of its own under the same id.
//! @pbt slips-if-removed the reverse sync leg moves nothing and every
//!   owner-side invariant stays green, because the owner-vs-oracle comparison
//!   is scoped to the OWNER's partition of the store by construction.
//!
//! ## Why this invariant has to exist
//! The composed harness scopes peer-authored ids out of the owner-vs-oracle
//! comparison (`ComposedSlice::foreign_ids`). That is a narrowing, not a hole,
//! ONLY because those ids are judged here instead. Delete this body and the
//! scoping becomes the hole.
//!
//! ## What it asserts, and what it deliberately does not
//! Membership on both peers, and the parent and CONTENT the peer wrote, read
//! off the owner. All three are order-independent, so a model with two
//! concurrent writers can predict them without re-implementing RGA tiebreaks.
//! Sibling ORDER among peer blocks is left to
//! `inv-two-instance-convergence`'s CRDT fixed point.
//!
//! Content matters here and is not covered elsewhere: a reverse leg that
//! carried the node but dropped its text would satisfy every membership and
//! parent claim and still lose the user's writing.

use holon_pbt_core::capabilities::RefSharedView;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutReceiverBackend;
use holon_pbt_core::capabilities::SutSqlProjection;
use holon_pbt_core::capabilities::SutTwoInstance;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvTwoWriterPeerWritesLand;

impl InvTwoWriterPeerWritesLand {
    pub const ID: InvariantId = InvariantId("inv-two-writer-peer-writes-land");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvTwoWriterPeerWritesLand
where
    R: RefSharedView,
    S: SutReceiverBackend + SutTwoInstance + SutSqlProjection + SutBackend,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let delivered = ref_.peer_writes_delivered();
        if delivered.is_empty() {
            return InvariantResult::Skipped(
                "the model records no peer write that a receiver→owner round has carried yet"
                    .into(),
            );
        }

        let receiver = sut.receiver_block_ids().await;
        let owner_blocks = sut.block_raw_snapshot().await;
        let owner: std::collections::BTreeSet<_> =
            owner_blocks.iter().map(|b| b.id.clone()).collect();
        let owner_authored = sut.locally_authored_ids(true).await;
        let receiver_authored = sut.locally_authored_ids(false).await;

        for (id, write) in &delivered {
            let parent = &write.parent;
            if !receiver_authored.contains(id) {
                return InvariantResult::Fail(format!(
                    "[inv-two-writer-peer-writes-land] {id} was authored on the RECEIVER by the \
                     model's account, but the receiver's own CRDT holds no node its peer created \
                     for it — the second writer's write never became a peer-authored op, so \
                     everything below would be judging the wrong block"
                ));
            }
            if !receiver.contains(id) {
                return InvariantResult::Fail(format!(
                    "[inv-two-writer-peer-writes-land] the receiver authored {id} but its own \
                     store does not hold it — the peer's Loro→SQL projection dropped a locally \
                     authored block"
                ));
            }
            if !owner.contains(id) {
                return InvariantResult::Fail(format!(
                    "[inv-two-writer-peer-writes-land] after a receiver→owner round the owner's \
                     store is MISSING the peer-authored block {id} (expected under {parent}); \
                     the reverse leg carried nothing, or the owner's projection did not \
                     materialize an imported node"
                ));
            }
            if owner_authored.contains(id) {
                return InvariantResult::Fail(format!(
                    "[inv-two-writer-peer-writes-land] {id} is present on the owner, but the \
                     OWNER's peer authored it — the owner minted its own block under the id the \
                     receiver used instead of admitting the receiver's, so presence here proves \
                     a collision rather than a delivery"
                ));
            }
            let children = sut.sorted_children(parent).await;
            if !children.contains(id) {
                return InvariantResult::Fail(format!(
                    "[inv-two-writer-peer-writes-land] the owner holds the peer-authored block \
                     {id} but NOT under {parent}: the parent's children are {children:?}. A \
                     merge that reparents a peer's block loses the structure the peer wrote"
                ));
            }
            let landed = owner_blocks
                .iter()
                .find(|b| &b.id == id)
                .expect("membership was just checked against this same snapshot");
            if landed.content != write.content {
                return InvariantResult::Fail(format!(
                    "[inv-two-writer-peer-writes-land] the peer-authored block {id} reached the \
                     owner under the right parent but says {:?}, not the {:?} the receiver \
                     wrote — a reverse leg that carries the node and drops its text loses the \
                     writing while satisfying every structural claim",
                    landed.content, write.content,
                ));
            }
        }

        InvariantResult::Ok
    }
}
