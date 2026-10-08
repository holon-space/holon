//! `inv-offered-ops-pass-the-write-tier` — the UI never offers an operation
//! the dispatcher's write tier then refuses (D46).
//!
//! @pbt oracle internal-consistency — the production offer path (`ops_of`)
//!   against the production dispatcher's write-tier verdict for the same
//!   operation on the same block
//! @pbt covers offer-matches-write-tier — the operations offered on a block
//!   whose document is homed in a `WriteTier::ReadOnly` file
//! @pbt slips-if-removed `ops_of` offers `set_field` and every other block
//!   operation on a `.cook` block, and each click is refused after the offer

use holon_pbt_core::capabilities::RefReadOnlyHomes;
use holon_pbt_core::capabilities::SutReadOnlyHomes;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvOfferedOpsPassTheWriteTier;

impl InvOfferedOpsPassTheWriteTier {
    pub const ID: InvariantId = InvariantId("inv-offered-ops-pass-the-write-tier");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvOfferedOpsPassTheWriteTier
where
    R: RefReadOnlyHomes,
    S: SutReadOnlyHomes,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, reference: &R, sut: &S) -> InvariantResult {
        let mut offered_then_refused = Vec::new();
        for block in reference.read_only_homed_blocks() {
            for (op, refusal) in sut.offered_ops_refused_by_write_tier(block.as_str()).await {
                offered_then_refused.push(format!("{block} {op}: {refusal}"));
            }
        }
        if offered_then_refused.is_empty() {
            return InvariantResult::Ok;
        }
        InvariantResult::Fail(format!(
            "{} operation(s) are offered on read-only-homed blocks, yet the dispatcher's write \
             tier refuses each one when it is dispatched (D46: an offer the dispatcher refuses is \
             an error): {offered_then_refused:#?}",
            offered_then_refused.len(),
        ))
    }
}
