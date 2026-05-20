//! `inv-value-fn-provider-identity`.
//!
//! Every intermediate ViewModel emission across a transition must carry
//! correct `StateToggle` values. A background task collects ALL emissions
//! from the reactive stream; this checks each `StateToggle`'s `current`
//! against the reference's task state, catching transient CDC-enrichment
//! glitches that a later structural re-render would mask (e.g.
//! `flatten_properties` mishandling a `Value::String` from the CDC path).
//!
//! Reads [`SutViewModel::drain_vm_emission_toggles`] (drains the emission
//! buffer, returns `(block_id, current)` per toggle) and the reference's
//! [`RefTaskState`] / [`RefBlockTree`]. Toggles for blocks the reference
//! doesn't track are skipped.

use holon_pbt_core::capabilities::{RefBlockTree, RefTaskState, SutViewModel};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

pub struct InvValueFnProviderIdentity;

impl InvValueFnProviderIdentity {
    pub const ID: InvariantId = InvariantId("inv-value-fn-provider-identity");
    const LABEL: &'static str = "inv-value-fn-provider-identity";
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvValueFnProviderIdentity
where
    R: RefTaskState + RefBlockTree,
    S: SutViewModel,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let toggles = sut.drain_vm_emission_toggles().await;

        let mut mismatches: Vec<String> = Vec::new();
        for (block_id, current) in &toggles {
            // Skip toggles for blocks the reference doesn't track (the inline
            // `block_state.blocks.get(..)` continue). `block_content` is `None`
            // only when the block is absent.
            if ref_.block_content(block_id).is_none() {
                continue;
            }
            let expected = ref_.task_state_of(block_id).unwrap_or_default();
            if current != &expected {
                mismatches.push(format!(
                    "block {block_id}: got '{current}', expected '{expected}'"
                ));
            }
        }

        if mismatches.is_empty() {
            InvariantResult::Ok
        } else {
            InvariantResult::Fail(format!(
                "[{label}] {n} intermediate ViewModel emission(s) carry a wrong StateToggle value — \
                 the CDC enrichment pipeline produced incorrect data visible as a UI glitch before \
                 the next structural re-render masks it.\n  {body}",
                label = Self::LABEL,
                n = mismatches.len(),
                body = mismatches.join("\n  "),
            ))
        }
    }
}
