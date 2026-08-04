//! Degraded-state bus for surfacing degradation to frontends (snapshot
//! save/load failures, rehydration errors, dead integrations).
//!
//! Unrelated to the block write path. Frontends subscribe via
//! [`DegradedSignalBus::subscribe`] and render banners; the subscription
//! carries the conditions already in effect plus a stream of later changes, so
//! a frontend that starts after a condition was raised still sees it.
//!
//! Producers emit and ignore lagged receivers — we prefer dropping
//! stale notifications over blocking the save worker.

use tokio::sync::broadcast;

/// Why a share is in a degraded state.
#[derive(Clone, Debug)]
pub enum ShareDegradedReason {
    /// Writing `<shared_tree_id>.loro` failed. The in-memory doc still
    /// holds the edit; the next commit will retry. String carries the
    /// underlying error.
    SnapshotSaveFailed(String),
    /// Reading `<shared_tree_id>.loro` failed at startup. The file has
    /// been renamed to `<path>.corrupt-<ts>` (carried in the string).
    /// The share is **not** registered — peer must re-accept to recover.
    SnapshotLoadFailed(String),
    /// Rehydration encountered an error after `load` succeeded — most
    /// commonly an advertiser-start failure on a non-idempotent code
    /// path. String carries the underlying error.
    RehydrationFailed(String),
    /// Projecting a shared doc's change into the SQL `block` table failed.
    /// Loro holds the change but SQL (which the UI reads) does not, so the
    /// two diverge until the next successful projection. The projection
    /// watermark is deliberately NOT advanced on failure, so the next
    /// commit retries the same diff. String carries the underlying error.
    SqlProjectionFailed(String),
    /// A shared doc tried to project a block whose id collides with a LIVE
    /// node in the recipient's global tree — i.e. it is trying to shadow a
    /// LOCAL block id (e.g. a malicious sharer naming a node `block:journals`).
    /// The projection is refused so it cannot clobber the recipient's own SQL
    /// row; the watermark is NOT advanced, so an honest later diff still
    /// projects. String carries the colliding block id.
    ForeignIdCollision(String),
    /// OrgMode initial-scan ingest failed for one or more vault files. The
    /// app stays up and the OTHER files keep syncing (the watch loop is
    /// armed), but the failed file(s) are NOT ingested until fixed — this is
    /// a visible degraded mode, not a silent sync death. String carries the
    /// aggregated per-file failure summary. `shared_tree_id` is the sentinel
    /// `"org-initial-scan"` (this is not tied to a shared doc).
    OrgIngestFailed(String),
    /// A block inside a shared subtree was edited, but its content could NOT be
    /// materialized to a dedicated on-disk org file (the mount is not yet a
    /// page-file, so the write-back layer cannot resolve a path). The edit is
    /// safe in Loro + SQL and syncs to peers, but disk org is stale until
    /// materialization is wired. Disclosed (not silently dropped) so the gap is
    /// visible. String carries the offending block id. `shared_tree_id` names
    /// the share.
    SharedSubtreeNotMaterialized(String),
    /// An MCP integration provider did not come up at boot — its sidecar
    /// command is missing/dead, or its `${VAR}` credentials are unresolved. The
    /// integration's `cc_*` cache tables are never created, so every page that
    /// queries them renders blank; disclosed so that blankness is attributable
    /// instead of looking like a healthy empty result. `shared_tree_id` carries
    /// the integration name (this is not tied to a shared doc).
    IntegrationConnectFailed { integration: String, error: String },
    /// An MCP integration provider needs an OAuth grant before it can connect.
    /// Same blank-page consequence as `IntegrationConnectFailed`, but the fix
    /// is a user action, so it carries the authorization URL.
    /// `shared_tree_id` carries the integration name.
    IntegrationNeedsAuth {
        integration: String,
        auth_url: String,
    },
}

impl ShareDegradedReason {
    /// A degradation that is an ongoing CONDITION (has a clearing path) rather
    /// than a transient event. Conditions are kept sticky by the bus and
    /// replayed to subscribers that arrive later; transient events are not,
    /// because nothing would ever clear them.
    pub fn condition_kind(&self) -> Option<&'static str> {
        match self {
            Self::IntegrationConnectFailed { .. } => Some("integration-connect-failed"),
            Self::IntegrationNeedsAuth { .. } => Some("integration-needs-auth"),
            Self::SnapshotSaveFailed(_)
            | Self::SnapshotLoadFailed(_)
            | Self::RehydrationFailed(_)
            | Self::SqlProjectionFailed(_)
            | Self::ForeignIdCollision(_)
            | Self::OrgIngestFailed(_)
            | Self::SharedSubtreeNotMaterialized(_) => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ShareDegraded {
    pub shared_tree_id: String,
    pub reason: ShareDegradedReason,
}

impl ShareDegraded {
    /// The sticky identity of this degradation, if it is a condition.
    pub fn condition_key(&self) -> Option<DegradedConditionKey> {
        self.reason
            .condition_kind()
            .map(|kind| DegradedConditionKey {
                subject: self.shared_tree_id.clone(),
                kind,
            })
    }
}

/// Identity of a sticky degraded condition. `subject` is the
/// `shared_tree_id` — for integrations, the integration name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DegradedConditionKey {
    pub subject: String,
    pub kind: &'static str,
}

/// A change to the degraded state.
#[derive(Clone, Debug)]
pub enum DegradedChange {
    Raised(ShareDegraded),
    Cleared(DegradedConditionKey),
}

impl DegradedChange {
    /// The raised event, or `None` when this change is a clear.
    pub fn raised(self) -> Option<ShareDegraded> {
        match self {
            Self::Raised(event) => Some(event),
            Self::Cleared(_) => None,
        }
    }
}

/// What a subscriber gets: the degraded conditions that are currently in
/// effect, plus the stream of subsequent changes.
pub struct DegradedSubscription {
    pub current: Vec<ShareDegraded>,
    pub changes: broadcast::Receiver<DegradedChange>,
}

/// Degraded-state bus: a sticky map of the currently-raised conditions plus a
/// broadcast of changes to it.
///
/// Stickiness removes the ordering contract between emitters and subscribers —
/// a condition raised during boot DI still reaches a window that launches
/// afterwards.
///
/// Senders never block. Slow subscribers get `RecvError::Lagged` on
/// their next `recv()` and must catch up — they do not stall producers.
pub struct DegradedSignalBus {
    tx: broadcast::Sender<DegradedChange>,
    /// Insertion order makes the replay in `subscribe` deterministic. N is one
    /// per degraded integration, so linear search beats a map.
    conditions: std::sync::Mutex<Vec<ShareDegraded>>,
}

impl DegradedSignalBus {
    /// Channel capacity. Chosen to absorb a short burst of failures
    /// (e.g., transient filesystem permission error on several shares
    /// at once) without any slow subscriber losing them.
    const CAPACITY: usize = 64;

    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(Self::CAPACITY);
        Self {
            tx,
            conditions: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Emit an event. Conditions are recorded as current state (replacing any
    /// prior entry with the same key); transient events are broadcast only.
    pub fn emit(&self, event: ShareDegraded) {
        if let Some(key) = event.condition_key() {
            let mut conditions = self.conditions.lock().unwrap();
            match conditions
                .iter_mut()
                .find(|c| c.condition_key().as_ref() == Some(&key))
            {
                Some(existing) => *existing = event.clone(),
                None => conditions.push(event.clone()),
            }
        }
        let _ = self.tx.send(DegradedChange::Raised(event));
    }

    /// Clear a condition. Broadcasts only if the condition was actually in
    /// effect, so consumers never see a phantom clear.
    pub fn clear(&self, key: &DegradedConditionKey) {
        let mut conditions = self.conditions.lock().unwrap();
        let before = conditions.len();
        conditions.retain(|c| c.condition_key().as_ref() != Some(key));
        let removed = conditions.len() != before;
        drop(conditions);
        if removed {
            let _ = self.tx.send(DegradedChange::Cleared(key.clone()));
        }
    }

    /// Live subscriber count. A bus that conditions are raised on with zero
    /// subscribers discloses to nobody — the frontend wiring is then mute even
    /// though every producer looks correctly wired, so this is the observable
    /// that makes "someone is listening" assertable.
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }

    pub fn subscribe(&self) -> DegradedSubscription {
        // Hold the conditions lock across `tx.subscribe()` so snapshot and
        // subscription are atomic against `emit`, which takes the same lock:
        // subscribing first can at worst deliver a condition twice (consumers
        // upsert by key), whereas snapshotting first could lose one entirely.
        let conditions = self.conditions.lock().unwrap();
        let changes = self.tx.subscribe();
        let current = conditions.clone();
        drop(conditions);
        DegradedSubscription { current, changes }
    }
}

impl Default for DegradedSignalBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raised(change: DegradedChange) -> ShareDegraded {
        change.raised().expect("expected Raised")
    }

    fn connect_failed(name: &str) -> ShareDegraded {
        ShareDegraded {
            shared_tree_id: name.into(),
            reason: ShareDegradedReason::IntegrationConnectFailed {
                integration: name.into(),
                error: "sidecar died".into(),
            },
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn transient_emit_without_subscribers_is_noop() {
        let bus = DegradedSignalBus::new();
        bus.emit(ShareDegraded {
            shared_tree_id: "s".into(),
            reason: ShareDegradedReason::SnapshotSaveFailed("disk full".into()),
        });
        assert!(bus.subscribe().current.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn subscriber_receives_event() {
        let bus = DegradedSignalBus::new();
        let mut sub = bus.subscribe();
        bus.emit(ShareDegraded {
            shared_tree_id: "abc".into(),
            reason: ShareDegradedReason::SnapshotLoadFailed("/tmp/x.corrupt-1".into()),
        });
        let ev = raised(sub.changes.recv().await.unwrap());
        assert_eq!(ev.shared_tree_id, "abc");
        assert!(matches!(
            ev.reason,
            ShareDegradedReason::SnapshotLoadFailed(ref p) if p.contains("corrupt")
        ));
    }

    /// The boot seam: integration failures are raised inside boot DI, before
    /// the window (the only consumer) exists.
    #[tokio::test(flavor = "current_thread")]
    async fn boot_time_conditions_reach_a_later_subscriber() {
        let bus = DegradedSignalBus::new();
        bus.emit(connect_failed("github"));
        bus.emit(ShareDegraded {
            shared_tree_id: "linear".into(),
            reason: ShareDegradedReason::IntegrationNeedsAuth {
                integration: "linear".into(),
                auth_url: "https://linear.app/oauth".into(),
            },
        });

        // window launches, strictly later
        let sub = bus.subscribe();

        let learned: Vec<String> = sub
            .current
            .iter()
            .map(|e| e.shared_tree_id.clone())
            .collect();
        assert_eq!(
            learned,
            vec!["github".to_string(), "linear".to_string()],
            "a subscriber that arrives after boot must still learn the current degraded conditions"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn re_raising_a_condition_replaces_rather_than_accumulates() {
        let bus = DegradedSignalBus::new();
        bus.emit(connect_failed("github"));
        bus.emit(ShareDegraded {
            shared_tree_id: "github".into(),
            reason: ShareDegradedReason::IntegrationConnectFailed {
                integration: "github".into(),
                error: "still dead".into(),
            },
        });
        let sub = bus.subscribe();
        assert_eq!(sub.current.len(), 1);
        assert!(matches!(
            sub.current[0].reason,
            ShareDegradedReason::IntegrationConnectFailed { ref error, .. } if error == "still dead"
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn clear_removes_the_condition_and_notifies() {
        let bus = DegradedSignalBus::new();
        bus.emit(connect_failed("github"));
        let mut sub = bus.subscribe();
        assert_eq!(sub.current.len(), 1);

        let key = DegradedConditionKey {
            subject: "github".into(),
            kind: "integration-connect-failed",
        };
        bus.clear(&key);

        assert!(matches!(
            sub.changes.recv().await.unwrap(),
            DegradedChange::Cleared(k) if k == key
        ));
        assert!(bus.subscribe().current.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn clearing_an_unraised_condition_broadcasts_nothing() {
        let bus = DegradedSignalBus::new();
        let mut sub = bus.subscribe();
        bus.clear(&DegradedConditionKey {
            subject: "github".into(),
            kind: "integration-connect-failed",
        });
        assert!(matches!(
            sub.changes.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn multiple_subscribers_all_see_events() {
        let bus = DegradedSignalBus::new();
        let mut sub1 = bus.subscribe();
        let mut sub2 = bus.subscribe();
        bus.emit(ShareDegraded {
            shared_tree_id: "x".into(),
            reason: ShareDegradedReason::RehydrationFailed("endpoint".into()),
        });
        assert_eq!(
            raised(sub1.changes.recv().await.unwrap()).shared_tree_id,
            "x"
        );
        assert_eq!(
            raised(sub2.changes.recv().await.unwrap()).shared_tree_id,
            "x"
        );
    }
}
