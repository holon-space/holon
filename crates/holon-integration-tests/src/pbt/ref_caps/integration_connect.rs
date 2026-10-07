//! `RefIntegrationConnect` — the integration connect-timing oracle read cap.
//!
//! @pbt kind ref
//! @pbt covers integration-op-routable-or-refused — whether the fake MCP
//!   integration's peer answers, from the drawn timing and the release.

use holon_pbt_core::capabilities::IntegrationConnectTiming;
use holon_pbt_core::capabilities::RefIntegrationConnect;

use super::super::reference_state::ReferenceState;

impl RefIntegrationConnect for ReferenceState {
    fn integration_connect_timing(&self) -> IntegrationConnectTiming {
        self.integration.timing
    }

    fn integration_peer_answers(&self) -> bool {
        match self.integration.timing {
            IntegrationConnectTiming::Instant => true,
            IntegrationConnectTiming::AfterSessionResolve => self.integration.released,
            IntegrationConnectTiming::Never => false,
        }
    }
}
