//! `inv-boundary-respected` — the receiver never holds a block outside the
//! audience the owner's policy sanctions.
//!
//! @pbt oracle differential — receiver store vs. the reference model's share
//!   audience.
//! @pbt covers two-instance-share — a cross-instance sharpening of
//!   `inv-audience-never-over-approximates`: that one asserts alignment inside
//!   ONE model, this one asserts it across a real second SUT.
//! @pbt slips-if-removed the acceptor admits an envelope whose membership proof
//!   is absent, lapsed, or for another principal, and the receiver silently
//!   holds state it was never granted.
//!
//! ## What this does NOT re-check
//! Sender-side op admissibility ("denied ops never enter a container doc") is
//! enforced by `check_boundary` at op dispatch, so those ops are absent from
//! the container by construction. This invariant covers the complementary half:
//! the MEMBERSHIP gate at container granularity (ADR 0028 §3 — containers
//! partition by audience). Duplicating the dispatch check here would test the
//! same mechanism twice and neither would test the acceptor.

use holon_pbt_core::capabilities::RefSharedView;
use holon_pbt_core::capabilities::SutReceiverBackend;
use holon_pbt_core::capabilities::SutTwoInstance;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvBoundaryRespected;

impl InvBoundaryRespected {
    pub const ID: InvariantId = InvariantId("inv-boundary-respected");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvBoundaryRespected
where
    R: RefSharedView,
    S: SutReceiverBackend + SutTwoInstance,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let audience = ref_.shared_audience();
        let receiver_principal = ref_.receiver_principal();
        if audience.members().contains(&receiver_principal) {
            // The receiver IS in the sanctioned audience — holding owner state
            // is exactly what the policy allows. Convergence is the other
            // invariant's job.
            return InvariantResult::Skipped(format!(
                "`{receiver_principal}` is in the sanctioned audience {:?}; nothing it holds can \
                 be out-of-audience",
                audience.members()
            ));
        }

        let owner = sut.owner_block_ids().await;
        let boot = sut.receiver_boot_block_ids().await;
        let receiver = sut.receiver_block_ids().await;
        let exclusive: std::collections::BTreeSet<_> = owner.difference(&boot).cloned().collect();
        if exclusive.is_empty() {
            return InvariantResult::Skipped(
                "the owner holds nothing the receiver did not boot with — no observable boundary \
                 to respect yet"
                    .into(),
            );
        }

        // Executed-witness: without a driven round, "the receiver holds nothing"
        // is true because nothing was attempted, which proves no enforcement.
        let witness = sut.sync_witness().await;
        if witness.rounds_run == 0 {
            return InvariantResult::Skipped(
                "no sync round has been driven — an unattempted transfer proves no boundary \
                 enforcement"
                    .into(),
            );
        }

        let held: Vec<_> = exclusive.intersection(&receiver).take(10).collect();
        if held.is_empty() {
            return InvariantResult::Ok;
        }
        InvariantResult::Fail(format!(
            "[inv-boundary-respected] `{receiver_principal}` is NOT in the sanctioned audience \
             {:?}, yet its store holds {} owner-exclusive block(s): {held:?}. {} round(s) ran \
             (pushed={} imported={}); the acceptor recorded refusals {:?} — an import that \
             happened anyway means the membership gate did not decide the import",
            audience.members(),
            exclusive.intersection(&receiver).count(),
            witness.rounds_run,
            witness.pushed,
            witness.imported,
            witness.refusals,
        ))
    }
}
