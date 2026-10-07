//! `inv-integration-op-routable-or-refused` — the integration's write is in
//! the dispatcher catalog once its peer answers, and refused naming the
//! integration while it does not.
//!
//! @pbt oracle correspondence
//! @pbt covers integration-connect-timing — an answering peer's write reaches
//!   the dispatcher; a silent peer's write is refused with an error naming the
//!   integration.
//! @pbt slips-if-removed a write to an integration that never connected would
//!   fail with a generic "no provider" error, or an integration connected
//!   after boot would never become dispatchable.

use std::collections::BTreeSet;
use std::time::Duration;
use std::time::Instant;

use holon_pbt_core::capabilities::RefIntegrationConnect;
use holon_pbt_core::capabilities::SutIntegrationConnect;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

use crate::fake_mcp_module::PROVIDER_NAME;
use crate::fake_mcp_module::WRITE_OP;
use crate::fake_mcp_module::WRITTEN_ENTITY;

pub struct InvIntegrationOpRoutableOrRefused;

impl InvIntegrationOpRoutableOrRefused {
    pub const ID: InvariantId = InvariantId("inv-integration-op-routable-or-refused");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvIntegrationOpRoutableOrRefused
where
    R: RefIntegrationConnect,
    S: SutIntegrationConnect,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let timing = ref_.integration_connect_timing();
        if !ref_.integration_peer_answers() {
            return match sut.dispatch_integration_op(WRITTEN_ENTITY, WRITE_OP).await {
                Err(refusal) if refusal.contains(PROVIDER_NAME) => InvariantResult::Ok,
                outcome => InvariantResult::Fail(format!(
                    "[inv-integration-op-routable-or-refused] `{PROVIDER_NAME}` drawn {timing:?} \
                     has not answered, so {WRITTEN_ENTITY}.{WRITE_OP} must be refused naming it; \
                     got {outcome:?}"
                )),
            };
        }
        // A peer released after boot connects in the background.
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let dispatcher: BTreeSet<String> = sut
                .integration_operation_surfaces(WRITTEN_ENTITY)
                .await
                .dispatcher;
            if dispatcher.contains(WRITE_OP) {
                return InvariantResult::Ok;
            }
            if Instant::now() >= deadline {
                return InvariantResult::Fail(format!(
                    "[inv-integration-op-routable-or-refused] `{PROVIDER_NAME}` drawn {timing:?} \
                     answers, but the dispatcher offers {WRITTEN_ENTITY} only {dispatcher:?}, \
                     not `{WRITE_OP}`"
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
