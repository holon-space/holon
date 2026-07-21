//! `inv-audience-never-over-approximates` — the REF-BOUNDARY tripwire for the
//! ADR 0028 C2/H3 directional-alignment invariant.
//!
//! **The invariant (ADR 0028 §H3, the directional restatement of the
//! unsatisfiable-as-literal C2):** *a block's EFFECTIVE audience never
//! OVER-approximates its POLICY audience at any observable point.* I.e. for
//! every block, `effective ⊆ policy` — the container a block is observably in
//! must never reach an audience the owner's policy does not sanction.
//!
//! - **Widening** a share (create-in-shared FIRST) can transiently *narrow*
//!   effective below policy — a missing duplicate, disclosed, never a leak.
//! - **Narrowing** a share (delete-from-shared FIRST) can transiently make a
//!   block invisible — disclosed, never silent. The FORBIDDEN state is the
//!   inverse: policy narrowed but the block still observable in the wider
//!   container. That is exactly what this oracle RED-catches.
//! - **Quiescent form at an epoch barrier:** membership = policy extension
//!   (effective = policy), the strongest form of `effective ⊆ policy`.
//!
//! @pbt oracle internal-consistency — ref-side structural tripwire on the
//!   share-migration guarantee (reads the ref audience overlay; reads no SUT).
//! @pbt covers sharing-audience — a block observably in a container whose
//!   policy audience no longer covers it (leak-direction violated).
//! @pbt slips-if-removed a wrong-order narrowing migration
//!   (delete-private-first / create-shared-last) leaves a block in a
//!   wider-than-policy container and the leak ships silently.
//!
//! ## Why a ref-only oracle (no SUT cap)
//!
//! There is no share machinery in prod yet, so there is no SUT observation to
//! differentiate against — the EFFECTIVE audience is the migration MODEL's own
//! container-membership. This is the same shape as
//! `inv-no-page-under-non-page`: it asserts the generator/migration guarantee
//! at the ref boundary, selecting on every composed draw (the ref always
//! provides `RefAudience`). It is vacuously Ok on a ref modeling no crossings
//! (`shared_block_ids()` empty — the default keystone), so it adds no false
//! RED. When real crossings land (Inc 4) the SUT side observes effective
//! audience via `list_loro_documents` / `inspect_loro_blocks` and this promotes
//! to a SUT⇄ref differential.
//!
//! `Needs RefAudience` only.

use holon_pbt_core::capabilities::RefAudience;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvAudienceNeverOverApproximates;

impl InvAudienceNeverOverApproximates {
    pub const ID: InvariantId = InvariantId("inv-audience-never-over-approximates");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvAudienceNeverOverApproximates
where
    R: RefAudience,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, _: &S) -> InvariantResult {
        let epoch = ref_.audience_epoch();
        let mut violations: Vec<String> = Vec::new();

        for block in ref_.shared_block_ids() {
            let effective = ref_.block_effective_audience(&block);
            let policy = ref_.block_policy_audience(&block);
            let leak = effective.over_approximation_of(&policy);
            if !leak.is_empty() {
                violations.push(format!(
                    "block `{}` effective audience OVER-approximates policy at epoch {epoch}: \
                     leaks to {:?} (effective={:?}, policy={:?}) — a block is observably in a \
                     container whose policy audience no longer covers it (ADR 0028 leak-direction \
                     violated: narrowing must delete-from-shared FIRST)",
                    block.id(),
                    leak,
                    effective.members(),
                    policy.members(),
                ));
            }
        }

        if violations.is_empty() {
            return InvariantResult::Ok;
        }
        InvariantResult::Fail(format!(
            "[inv-audience-never-over-approximates] {} block(s) over-approximate their policy \
             audience: {:?}",
            violations.len(),
            violations.iter().take(10).collect::<Vec<_>>(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::collections::BTreeSet;

    use holon_pbt_core::capabilities::Audience;
    use holon_pbt_core::capabilities::EntityUri;

    use super::*;

    /// Minimal `RefAudience` modeling a fixed (policy, effective) audience per
    /// block at a given epoch. Only the four methods the invariant calls are
    /// meaningful.
    #[derive(Default)]
    struct RefStub {
        epoch: u64,
        policy: BTreeMap<EntityUri, Audience>,
        effective: BTreeMap<EntityUri, Audience>,
    }
    impl RefStub {
        fn new() -> Self {
            Self::default()
        }
        fn at_epoch(mut self, e: u64) -> Self {
            self.epoch = e;
            self
        }
        /// A block shared to `effective` members under an owner policy of
        /// `policy` members.
        fn block(mut self, id: &str, policy: &[&str], effective: &[&str]) -> Self {
            let uri = EntityUri::block(id);
            self.policy
                .insert(uri.clone(), Audience::from_members(policy.iter().copied()));
            self.effective
                .insert(uri, Audience::from_members(effective.iter().copied()));
            self
        }
    }
    impl RefAudience for RefStub {
        fn audience_epoch(&self) -> u64 {
            self.epoch
        }
        fn shared_block_ids(&self) -> BTreeSet<EntityUri> {
            // Union of blocks carrying a non-local policy or effective audience.
            self.policy
                .iter()
                .chain(self.effective.iter())
                .filter(|(_, a)| !a.is_local_only())
                .map(|(id, _)| id.clone())
                .collect()
        }
        fn block_policy_audience(&self, block: &EntityUri) -> Audience {
            self.policy.get(block).cloned().unwrap_or_default()
        }
        fn block_effective_audience(&self, block: &EntityUri) -> Audience {
            self.effective.get(block).cloned().unwrap_or_default()
        }
    }

    async fn check(stub: &RefStub) -> InvariantResult {
        Invariant::<RefStub, ()>::check(&InvAudienceNeverOverApproximates, stub, &()).await
    }

    #[tokio::test]
    async fn no_crossings_is_vacuously_ok() {
        // Default keystone: nothing shared.
        let stub = RefStub::new();
        assert!(matches!(check(&stub).await, InvariantResult::Ok));
    }

    #[tokio::test]
    async fn effective_equals_policy_is_ok() {
        // Quiescent form: membership = policy extension.
        let stub = RefStub::new().block("b", &["alice", "bob"], &["alice", "bob"]);
        assert!(matches!(check(&stub).await, InvariantResult::Ok), "{:?}", {
            check(&stub).await
        });
    }

    #[tokio::test]
    async fn narrowing_transient_invisibility_is_ok() {
        // Delete-from-shared FIRST: effective narrower than policy — no leak.
        let stub = RefStub::new().block("b", &["alice", "bob"], &["alice"]);
        assert!(matches!(check(&stub).await, InvariantResult::Ok));
    }

    #[tokio::test]
    async fn over_approximation_leaks_and_is_caught() {
        // The FORBIDDEN state — wrong-order narrowing: policy dropped `bob` but
        // the block is still observably in a container that reaches `bob`.
        let stub = RefStub::new()
            .at_epoch(3)
            .block("secret", &["alice"], &["alice", "bob"]);
        match check(&stub).await {
            InvariantResult::Fail(m) => {
                assert!(
                    m.contains("`secret`") && m.contains("bob") && m.contains("epoch 3"),
                    "must name the leaking block, the leaked member, and the epoch: {m}"
                );
            }
            other => panic!("an over-approximating effective audience must FAIL, got {other:?}"),
        }
    }
}
