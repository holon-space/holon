//! `inv-integration-operations-reach-profiles` — once the integration's peer
//! answers, every entity it mirrors offers on a rendered row exactly the
//! operations the dispatcher accepts for it.
//!
//! @pbt oracle correspondence
//! @pbt covers integration-connect-timing — for each mirrored entity, the
//!   profile resolver's operation names, and those a row re-resolved on the
//!   profile signal carries, equal the dispatcher catalog's.
//! @pbt slips-if-removed an integration connected after boot would be
//!   dispatchable while its rendered rows offered none of its operations.

use std::collections::BTreeSet;

use holon_pbt_core::capabilities::RefIntegrationConnect;
use holon_pbt_core::capabilities::SutIntegrationConnect;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

use super::operation_surfaces_agree::operation_surfaces_agree;
use crate::fake_mcp_module::ENTITIES;
use crate::fake_mcp_module::WRITE_OP;
use crate::fake_mcp_module::WRITTEN_ENTITY;

pub struct InvIntegrationOperationsReachProfiles;

impl InvIntegrationOperationsReachProfiles {
    pub const ID: InvariantId = InvariantId("inv-integration-operations-reach-profiles");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvIntegrationOperationsReachProfiles
where
    R: RefIntegrationConnect,
    S: SutIntegrationConnect,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        if !ref_.integration_peer_answers() {
            return InvariantResult::Ok;
        }
        for entity in ENTITIES {
            let required: BTreeSet<String> = if entity == WRITTEN_ENTITY {
                BTreeSet::from([WRITE_OP.to_string()])
            } else {
                BTreeSet::new()
            };
            if sut
                .integration_operation_surfaces(entity)
                .await
                .rerendered
                .is_none()
            {
                return InvariantResult::Fail(format!(
                    "[inv-integration-operations-reach-profiles] '{entity}' (peer drawn {:?}): no \
                     rendered row of it is followed, so its rendered operations go unchecked",
                    ref_.integration_connect_timing()
                ));
            }
            if let Err(surfaces) = operation_surfaces_agree(&required, async || {
                sut.integration_operation_surfaces(entity).await
            })
            .await
            {
                return InvariantResult::Fail(format!(
                    "[inv-integration-operations-reach-profiles] '{entity}' (peer drawn {:?}): a \
                     rendered row offers different operations than the dispatcher accepts \
                     (required {required:?})\n  dispatcher: {:?}\n  profile:    {:?}\n  \
                     rerendered: {:?}",
                    ref_.integration_connect_timing(),
                    surfaces.dispatcher,
                    surfaces.profile,
                    surfaces.rerendered
                ));
            }
        }
        InvariantResult::Ok
    }
}
