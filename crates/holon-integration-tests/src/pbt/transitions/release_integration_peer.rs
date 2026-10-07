//! Transition: the gated fake MCP peer starts answering, after the session
//! resolved.
//!
//! @pbt rung external
//!   open the gate of the loopback MCP server the session's integration
//!   module connects to (`SutIntegrationConnect::release_integration_peer`).
//! @pbt covers integration-connect-timing — an integration that connects
//!   after boot reaches the dispatcher and the profiles

use holon_pbt_core::RequiredWiring;
use holon_pbt_core::StorageAdapter;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::IntegrationConnectTiming;
use holon_pbt_core::capabilities::RefIntegrationConnect;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutIntegrationConnect;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("the integration peer is released")]
pub struct ReleaseIntegrationPeer;

fn held(state: &ReferenceState) -> bool {
    state.integration.timing == IntegrationConnectTiming::AfterSessionResolve
        && !state.integration.released
}

impl TransitionFactory<ReferenceState> for ReleaseIntegrationPeer {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn required_wiring() -> RequiredWiring {
        RequiredWiring::HasStorage(StorageAdapter::Turso)
    }

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        Self::preconditions(&ReleaseIntegrationPeer, state)
            .map(|_| (40, Just(ReleaseIntegrationPeer).boxed()))
    }
}

impl TransitionRef<ReferenceState> for ReleaseIntegrationPeer {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(held(state), Reason::IntegrationPeerNotHeld),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.integration.released = true;
    }
}

crate::cap_transition! {
    ReleaseIntegrationPeer: SutIntegrationConnect,
    where R: [RefLifecycle + RefIntegrationConnect],
    |_me, _state, sut| {
        sut.release_integration_peer().await;
    }
    sql_budget: |_me, _state| {
        // The release only opens the peer's gate; the connect and initial
        // sync it unblocks run in the background.
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 64,
        }
    }
}
