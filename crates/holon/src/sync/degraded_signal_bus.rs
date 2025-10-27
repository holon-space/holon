//! Broadcast channel for surfacing share-persistence degradation to
//! frontends (snapshot save/load failures, rehydration errors).
//!
//! This is intentionally separate from [`event_bus::EventBus`], whose
//! [`EventKind`] enum is closed on block-level operations. Frontends
//! subscribe via [`DegradedSignalBus::subscribe`] and render banners.
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
