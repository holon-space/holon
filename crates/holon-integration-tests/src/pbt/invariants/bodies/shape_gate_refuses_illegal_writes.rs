//! `inv-shape-gate-refuses-illegal-writes` — a tagged subtree is a write
//! BOUNDARY.
//!
//! @pbt oracle model — the outcome the model decides by running the decision
//!   block adapter on its own post-edit subtree
//! @pbt covers shape-write-boundary — a Holon-side edit of a `decision`
//!   subtree, legal or not, one gesture at a time
//! @pbt slips-if-removed a write that names a non-option as `chosen` lands, and
//!   the store holds a decision no reader can parse
//! @pbt slips-if-removed a gesture that is legal only as a whole (a new
//!   `choose` together with a ruling of that size) is refused op by op
//!
//! The block state itself is judged by `inv-blocks-match-ref`, which compares
//! the store with the model's subtree. This invariant judges the verdict: that
//! each edit was accepted or refused as the model decided, and that a refusal
//! names the rule.

use holon_pbt_core::capabilities::ExpectedShapeOutcome;
use holon_pbt_core::capabilities::RefShapeEdits;
use holon_pbt_core::capabilities::SutShapeEdit;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvShapeGateRefusesIllegalWrites;

impl InvShapeGateRefusesIllegalWrites {
    pub const ID: InvariantId = InvariantId("inv-shape-gate-refuses-illegal-writes");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvShapeGateRefusesIllegalWrites
where
    R: RefShapeEdits,
    S: SutShapeEdit,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, reference: &R, sut: &S) -> InvariantResult {
        let expected = reference.expected_shape_outcomes();
        let actual = sut.shape_edit_outcomes().await;
        if expected.is_empty() && actual.is_empty() {
            return InvariantResult::Skipped("no tagged-subtree edit was drawn".into());
        }
        if expected.len() != actual.len() {
            return InvariantResult::Fail(format!(
                "the model decided {} tagged-subtree edit(s) but the SUT recorded {}",
                expected.len(),
                actual.len()
            ));
        }
        for (i, (want, got)) in expected.iter().zip(&actual).enumerate() {
            match (want, got) {
                (ExpectedShapeOutcome::Applied, Ok(())) => {}
                (ExpectedShapeOutcome::Applied, Err(e)) => {
                    return InvariantResult::Fail(format!(
                        "edit #{i}: the resulting decision is legal, yet the write was refused: \
                         {e}"
                    ));
                }
                (ExpectedShapeOutcome::Refused { rule }, Ok(())) => {
                    return InvariantResult::Fail(format!(
                        "edit #{i}: expected refusal {rule}, got Ok — the write landed and the \
                         store now holds a decision the block adapter refuses"
                    ));
                }
                (ExpectedShapeOutcome::Refused { rule }, Err(e)) => {
                    if !e.contains(&format!("{rule}:")) && !e.contains(&format!("rule {rule}")) {
                        return InvariantResult::Fail(format!(
                            "edit #{i}: refused, but not for rule {rule}: {e}"
                        ));
                    }
                }
            }
        }
        InvariantResult::Ok
    }
}
