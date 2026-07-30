//! Broadcast channel for surfacing share-persistence degradation to
//! frontends (snapshot save/load failures, rehydration errors).
//!
//! This is a dedicated broadcast for degradation signals, unrelated to the
//! block write path. Frontends subscribe via [`DegradedSignalBus::subscribe`]
//! and render banners.
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

#[derive(Clone, Debug)]
pub struct ShareDegraded {
    pub shared_tree_id: String,
    pub reason: ShareDegradedReason,
}

/// Broadcast channel for `ShareDegraded` events.
///
/// Senders never block. Slow subscribers get `RecvError::Lagged` on
/// their next `recv()` and must catch up — they do not stall producers.
pub struct DegradedSignalBus {
    tx: broadcast::Sender<ShareDegraded>,
}

impl DegradedSignalBus {
    /// Channel capacity. Chosen to absorb a short burst of failures
    /// (e.g., transient filesystem permission error on several shares
    /// at once) without any slow subscriber losing them.
    const CAPACITY: usize = 64;

    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(Self::CAPACITY);
        Self { tx }
    }

    /// Emit an event. If there are no subscribers, the event is
    /// discarded — that's the intended broadcast semantics.
    pub fn emit(&self, event: ShareDegraded) {
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ShareDegraded> {
        self.tx.subscribe()
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

    #[tokio::test(flavor = "current_thread")]
    async fn emit_without_subscribers_is_noop() {
        let bus = DegradedSignalBus::new();
        bus.emit(ShareDegraded {
            shared_tree_id: "s".into(),
            reason: ShareDegradedReason::SnapshotSaveFailed("disk full".into()),
        });
    }

    #[tokio::test(flavor = "current_thread")]
    async fn subscriber_receives_event() {
        let bus = DegradedSignalBus::new();
        let mut rx = bus.subscribe();
        bus.emit(ShareDegraded {
            shared_tree_id: "abc".into(),
            reason: ShareDegradedReason::SnapshotLoadFailed("/tmp/x.corrupt-1".into()),
        });
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.shared_tree_id, "abc");
        assert!(matches!(
            ev.reason,
            ShareDegradedReason::SnapshotLoadFailed(ref p) if p.contains("corrupt")
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn multiple_subscribers_all_see_events() {
        let bus = DegradedSignalBus::new();
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();
        bus.emit(ShareDegraded {
            shared_tree_id: "x".into(),
            reason: ShareDegradedReason::RehydrationFailed("endpoint".into()),
        });
        assert_eq!(rx1.recv().await.unwrap().shared_tree_id, "x");
        assert_eq!(rx2.recv().await.unwrap().shared_tree_id, "x");
    }
}
