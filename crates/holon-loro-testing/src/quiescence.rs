//! Loro sync-quiescence helper, co-located with the Loro SUT it serves.
//!
//! Waits until the `LoroSyncController`'s `last_synced` watermark matches the
//! global doc's current `oplog_frontiers()`. Used by [`crate::LoroSut`]'s
//! peer-sync ops and by the central `TestEnvironment::wait_for_loro_quiescence`
//! (which imports it back from this crate).

use std::sync::Arc;

use holon::sync::{LoroDocumentStore, LoroSyncControllerHandle};
use tokio::sync::RwLock;

/// Wait until the `LoroSyncController`'s `last_synced` watermark matches the
/// global doc's current `oplog_frontiers()`, bounded by `timeout`.
pub async fn wait_for_loro_quiescence_on(
    handle: &Arc<LoroSyncControllerHandle>,
    doc_store: &Arc<RwLock<LoroDocumentStore>>,
    timeout: std::time::Duration,
) {
    use tracing::field;
    let span = tracing::info_span!(
        "wait_for_loro_quiescence",
        timeout_ms = timeout.as_millis() as u64,
        attempts = field::Empty,
        timed_out = field::Empty,
    );
    let _enter = span.enter();
    let deadline = tokio::time::Instant::now() + timeout;
    let mut attempts: u32 = 0;
    loop {
        attempts += 1;
        let current = {
            let store = doc_store.read().await;
            store
                .get_global_doc()
                .await
                .expect("wait_for_loro_quiescence: get_global_doc failed")
                .doc()
                .oplog_frontiers()
        };
        if handle.last_synced_frontiers() == current {
            span.record("attempts", attempts);
            span.record("timed_out", false);
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            span.record("attempts", attempts);
            span.record("timed_out", true);
            eprintln!("[wait_for_loro_quiescence] timeout after {:?}", timeout);
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
