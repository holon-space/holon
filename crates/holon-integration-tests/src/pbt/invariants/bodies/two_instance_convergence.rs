//! `inv-two-instance-convergence` — after a share and at least one sync round,
//! the RECEIVER instance actually holds the owner's state.
//!
//! @pbt oracle differential — receiver projections vs. owner projections, gated
//!   by the reference model's share + round count.
//! @pbt covers two-instance-share, two-instance-sync — a shared vault that
//!   never reaches the receiver, and a receiver whose org writeback drops the
//!   received blocks.
//! @pbt slips-if-removed the transport, acceptor, or orchestrator silently
//!   moves nothing and every other invariant stays green (both instances are
//!   independently self-consistent).
//!
//! ## The three layers
//! 1. **Store.** Every OWNER-EXCLUSIVE block id (`owner − receiver-at-boot`) is
//!    in the receiver's store. Boot-shared ids (the programmatic default
//!    layout, minted under the same fixed ids on both sides) are subtracted, so
//!    presence proves transport rather than coincidence.
//! 2. **Org writeback.** The receiver's `.org` files carry every
//!    owner-exclusive id the OWNER'S OWN org files carry. Comparing against the
//!    owner's org set (not the store set) makes this the IDENTITY landing-zone
//!    mapping the whole-vault self-device case specifies: both sides run the
//!    same writeback placement rule over the same tree, so the same blocks must
//!    reach disk.
//! 3. **CRDT.** The two documents reach the same state under a pairwise
//!    fork-and-sync fixed point.
//!
//! ## The unshared branch has teeth too
//! With nothing shared, the receiver must hold NO owner-exclusive block — the
//! Inc0 negative. That branch is only meaningful beside the executed-witness
//! (`SyncRoundWitness`) proving `sync_once` ran and consulted the transport, so
//! this body demands a witness and FAILS on a vacuous zero-round claim.

use holon_pbt_core::capabilities::RefSharedView;
use holon_pbt_core::capabilities::SutReceiverBackend;
use holon_pbt_core::capabilities::SutTwoInstance;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvTwoInstanceConvergence;

impl InvTwoInstanceConvergence {
    pub const ID: InvariantId = InvariantId("inv-two-instance-convergence");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvTwoInstanceConvergence
where
    R: RefSharedView,
    S: SutReceiverBackend + SutTwoInstance,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let receiver = sut.receiver_block_ids().await;
        let boot = sut.receiver_boot_block_ids().await;
        let witness = sut.sync_witness().await;
        // Basis for "what must have arrived": what the LAST round could carry —
        // not the owner's live set. A block created after that round has not been
        // offered to the receiver yet. On the negative branch the live set IS the
        // right basis: any owner-exclusive id on the receiver is a leak whenever
        // it appeared.
        let expects_convergence = ref_.is_shared() && ref_.owner_to_receiver_rounds() > 0;
        let owner = if expects_convergence {
            sut.owner_block_ids_at_last_round().await
        } else {
            sut.owner_block_ids().await
        };
        let exclusive: std::collections::BTreeSet<_> = owner.difference(&boot).cloned().collect();

        if exclusive.is_empty() {
            return InvariantResult::Skipped(
                "the owner holds no block the receiver did not already have at boot — nothing \
                 whose presence could prove or disprove transport"
                    .into(),
            );
        }

        // PROVENANCE, not co-presence. An id can be on both sides because both
        // sides minted it: the fixed boot ids do, and so does any deterministic
        // rule (the journal day block under the fixed clock) whose async firing
        // can land after the receiver's boot snapshot was taken — which is
        // exactly how a correct system used to read as a leak here. Only a
        // block the receiver's own peer never authored can have crossed.
        let receiver_authored = sut.locally_authored_ids(false).await;
        let leaked: Vec<_> = exclusive
            .intersection(&receiver)
            .filter(|id| !receiver_authored.contains(*id))
            .take(10)
            .collect();

        if !expects_convergence {
            // NEGATIVE branch. Vacuous unless the sync path actually ran.
            if witness.rounds_run == 0 {
                return InvariantResult::Skipped(
                    "no sync round has been driven yet — the nothing-crossed assertion would be \
                     vacuous"
                        .into(),
                );
            }
            if witness.containers_visited == 0 {
                return InvariantResult::Fail(format!(
                    "[inv-two-instance-convergence] {} sync round(s) ran but visited ZERO \
                     containers — the orchestrator did not walk the replication set, so \
                     'nothing crossed' proves nothing about the acceptor",
                    witness.rounds_run
                ));
            }
            if !leaked.is_empty() {
                return InvariantResult::Fail(format!(
                    "[inv-two-instance-convergence] the receiver holds owner-exclusive blocks \
                     {leaked:?} — which its OWN peer never authored — although the model says \
                     shared={} rounds={}: state crossed without an authorized, \
                     model-acknowledged share (unauthorized: {:?}, refusals: {:?})",
                    ref_.is_shared(),
                    ref_.owner_to_receiver_rounds(),
                    witness.unauthorized,
                    witness.refusals,
                ));
            }
            return InvariantResult::Ok;
        }

        // POSITIVE branch: shared + at least one owner→receiver round.
        let missing: Vec<_> = exclusive.difference(&receiver).take(10).collect();
        if !missing.is_empty() {
            return InvariantResult::Fail(format!(
                "[inv-two-instance-convergence] after {} owner→receiver round(s) over a shared \
                 vault, the receiver's store is MISSING {} of {} owner-exclusive block(s) that \
                 EXISTED WHEN THAT ROUND RAN: \
                 {missing:?} (last round: pushed={} imported={} refusals={:?} unmounted={:?})",
                ref_.owner_to_receiver_rounds(),
                exclusive.difference(&receiver).count(),
                exclusive.len(),
                witness.pushed,
                witness.imported,
                witness.refusals,
                witness.unmounted,
            ));
        }

        // Layer 2: receiver org writeback, against the owner's own org set.
        let owner_org = sut.owner_org_block_ids().await;
        let receiver_org = sut.receiver_org_block_ids().await;
        let expected_org: std::collections::BTreeSet<_> =
            owner_org.intersection(&exclusive).cloned().collect();
        let missing_org: Vec<_> = expected_org.difference(&receiver_org).take(10).collect();
        if !missing_org.is_empty() {
            return InvariantResult::Fail(format!(
                "[inv-two-instance-convergence] the receiver's STORE converged but its ORG files \
                 are missing {} of {} received block(s) the owner's own org carries: \
                 {missing_org:?} — received state that never reaches disk is lost on restart",
                expected_org.difference(&receiver_org).count(),
                expected_org.len(),
            ));
        }

        // Layer 3: CRDT fixed point.
        match sut.crdt_converged().await {
            Some(false) => InvariantResult::Fail(
                "[inv-two-instance-convergence] the two instances' Loro documents do NOT reach a \
                 common state under a pairwise fork-and-sync fixed point, although the projected \
                 block sets agree — a projection is masking a divergent CRDT history"
                    .into(),
            ),
            _ => InvariantResult::Ok,
        }
    }
}
