//! Loro sync-quiescence helper, co-located with the Loro SUT it serves.
//!
//! @pbt kind cap-plumbing
//! @pbt covers loro-slice (full) — settle barrier before a post-merge read.
//!   Fails LOUD on timeout (returns `Err` + `tracing::error!`), never a silent
//!   return — a never-quiescing sync is surfaced to the caller (F7).
//!
//! Waits until the `LoroSyncController`'s `last_synced` watermark matches the
//! global doc's current `oplog_frontiers()`. Used by [`crate::LoroSut`]'s
//! peer-sync ops and by the central `TestEnvironment::wait_for_loro_quiescence`
//! (which imports it back from this crate).

use std::sync::Arc;

use holon_loro::DocScope;
use holon_loro::LoroDocumentStore;
use holon_loro::LoroSyncControllerHandle;
use tokio::sync::RwLock;

/// Wait until the `LoroSyncController`'s `last_synced` watermark matches the
/// global doc's current `oplog_frontiers()`, bounded by `timeout`.
pub async fn wait_for_loro_quiescence_on(
    handle: &Arc<LoroSyncControllerHandle>,
    doc_store: &Arc<RwLock<LoroDocumentStore>>,
    timeout: std::time::Duration,
) -> Result<(), String> {
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
                .get_doc(DocScope::Global)
                .await
                .expect("wait_for_loro_quiescence: get_doc(Global) failed")
                .doc()
                .oplog_frontiers()
        };
        if handle.last_synced_frontiers() == current {
            span.record("attempts", attempts);
            span.record("timed_out", false);
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            span.record("attempts", attempts);
            span.record("timed_out", true);
            // Fail loud: emit through tracing (so `inv-no-observed-errors` and
            // the span collector can SEE it — an `eprintln` bypassed both) AND
            // return `Err` so the caller cannot mistake a hung sync for a
            // settled one (F7).
            let msg = format!(
                "loro sync never quiesced within {timeout:?} after {attempts} attempts: \
                 controller last_synced_frontiers != global oplog_frontiers"
            );
            tracing::error!(target: "holon_loro", "{msg}");
            return Err(msg);
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
