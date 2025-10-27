//! Capability-probe gating for MCP freshness mechanisms.
//!
//! After connect, the server's advertised `capabilities.resources` decides
//! whether `resources/subscribe` is attempted at all. Polling is an explicit
//! per-entity opt-in (`sync.interval` in the sidecar YAML) and runs whether or
//! not the server supports subscriptions — setting an interval IS the explicit
//! configuration.
//!
//! Decision matrix (per entity with a sync strategy):
//!
//! | server subscribe | `sync.interval` | plan                                 |
//! |------------------|-----------------|--------------------------------------|
//! | yes              | unset           | subscribe only                       |
//! | yes              | set             | subscribe + poll (explicit)          |
//! | no               | set             | poll only (disclosed WARN)           |
//! | no               | unset           | initial sync only (disclosed WARN: cache goes stale) |

use std::time::Duration;

use rmcp::model::ResourcesCapability;

/// Server resource capabilities probed from `peer_info()` after connect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbedResourceCapabilities {
    /// Server advertises `resources.subscribe`.
    pub subscribe: bool,
    /// Server advertises `resources.listChanged`.
    pub list_changed: bool,
}

impl ProbedResourceCapabilities {
    /// Parse the advertised capabilities at the connect boundary.
    /// Absent `resources` capability (or absent flags) means unsupported.
    pub fn from_server(resources: Option<&ResourcesCapability>) -> Self {
        Self {
            subscribe: resources.and_then(|r| r.subscribe).unwrap_or(false),
            list_changed: resources.and_then(|r| r.list_changed).unwrap_or(false),
        }
    }
}

/// How an entity's cache stays fresh after the initial sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshnessPlan {
    /// Server pushes `resources/updated` notifications.
    Subscribe,
    /// Notifications plus an explicitly configured poll cadence.
    SubscribeAndPoll(Duration),
    /// No subscribe capability — poll at the configured cadence.
    PollOnly(Duration),
    /// No subscribe capability and no interval — cache is stale after the
    /// initial sync. Disclosed with a WARN at connect.
    InitialSyncOnly,
}

/// The probe-gating decision: subscribe capability x configured interval.
pub fn freshness_plan(subscribe_supported: bool, interval: Option<Duration>) -> FreshnessPlan {
    match (subscribe_supported, interval) {
        (true, None) => FreshnessPlan::Subscribe,
        (true, Some(d)) => FreshnessPlan::SubscribeAndPoll(d),
        (false, Some(d)) => FreshnessPlan::PollOnly(d),
        (false, None) => FreshnessPlan::InitialSyncOnly,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    #[test]
    fn subscribe_capable_without_interval_subscribes_only() {
        assert_eq!(freshness_plan(true, None), FreshnessPlan::Subscribe);
    }

    #[test]
    fn subscribe_capable_with_interval_gets_both() {
        assert_eq!(
            freshness_plan(true, Some(MIN)),
            FreshnessPlan::SubscribeAndPoll(MIN)
        );
    }

    #[test]
    fn no_subscribe_with_interval_polls_only() {
        assert_eq!(
            freshness_plan(false, Some(MIN)),
            FreshnessPlan::PollOnly(MIN)
        );
    }

    #[test]
    fn no_subscribe_without_interval_is_initial_sync_only() {
        assert_eq!(freshness_plan(false, None), FreshnessPlan::InitialSyncOnly);
    }

    #[test]
    fn probe_parses_advertised_capabilities() {
        let caps = ResourcesCapability {
            subscribe: Some(true),
            list_changed: None,
        };
        let probed = ProbedResourceCapabilities::from_server(Some(&caps));
        assert!(probed.subscribe);
        assert!(!probed.list_changed);

        let absent = ProbedResourceCapabilities::from_server(None);
        assert!(!absent.subscribe);
        assert!(!absent.list_changed);
    }
}
