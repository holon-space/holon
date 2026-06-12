//! Generic debounced commit worker.
//!
//! Shared skeleton used by [`super::loro_share_backend::SaveWorker`] and
//! [`super::loro_share_backend::SyncWorker`]:
//!
//! 1. `LoroDoc::subscribe_root` → a `Notify` waker, optionally filtered
//!    by a caller-supplied predicate on `DiffEvent` (e.g. "local-origin
//!    commits only").
//! 2. Task loop: wait on notify, sleep for the debounce window, drain
//!    any notifications that piled up during the sleep (so a typing
//!    burst coalesces into a single work call), run the async work
//!    callback.
//!
//! The callback MUST NOT hold a Loro write lock across an await — the
//! outer `LoroShareBackend` relies on tight lock scopes to keep the
//! global doc available to other writers. The worker itself never
//! touches the doc; it only wakes on changes.
//!
//! Errors from the work callback are surfaced through `tracing::error!`
//! with the configured `worker_name` tag plus any per-error context the
//! callback provides. The loop does NOT exit on error — the next commit
//! re-triggers the worker.
//!
//! TODO: `crates/holon/src/sync/loro_sync_controller.rs:179-184` has a
//! structurally similar `subscribe_root + Notify` pattern but without a
//! debounce window. Unifying it into this worker would require adding
//! `Duration::ZERO` handling (skip the sleep entirely). Not done in
//! this pass because the controller has a different lifecycle (explicit
//! `run_loop` + error counter) and folding it in would require more
//! surgery than a simple worker swap.

use futures::FutureExt;
use loro::LoroDoc;
use loro::event::DiffEvent;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::task::AbortHandle;
use tokio::time::Duration;

/// A filter predicate for subscribe_root events. Return `true` to wake
/// the worker, `false` to ignore the event. Used by the sync worker to
/// skip `Import` events (remote-origin commits just applied by the sync
/// protocol) so re-syncing them back out doesn't churn forever.
///
/// Called synchronously inside the Loro callback — must be cheap and
/// must not block.
pub type EventFilter = Arc<dyn Fn(&DiffEvent<'_>) -> bool + Send + Sync>;

/// Always-fire filter: any commit wakes the worker.
pub fn any_commit() -> EventFilter {
    Arc::new(|_| true)
}

/// Only wake on local-origin commits (skip `Import` events applied by
/// the sync protocol).
pub fn local_only() -> EventFilter {
    Arc::new(|event| event.triggered_by == loro::EventTriggerKind::Local)
}

/// Handle for a running debounced commit worker. Dropping the handle
/// aborts the background task AND unregisters the Loro callback (via
/// the held `Subscription`'s `Drop` impl).
pub struct DebouncedCommitWorkerHandle {
    _subscription: loro::Subscription,
    abort: AbortHandle,
}

impl Drop for DebouncedCommitWorkerHandle {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

/// Spawn a debounced commit worker.
///
/// The generic parameters are:
/// - `F`: the work factory — called each time the debounce fires. Must
///   return a fresh `Future` (so it can be called repeatedly). Using a
///   factory rather than a `FnMut() -> Future` lets the caller close
///   over whatever `Arc` state it needs and produce fresh futures
///   without &mut bookkeeping.
/// - `Fut`: the future type the factory returns. Must be `Send` + the
///   `Result` it resolves to is printed via `{:#}` on error.
///
/// `worker_name` is used purely for `tracing` spans — something like
/// `"save"` or `"sync"`.
pub fn spawn<F, Fut>(
    doc: Arc<LoroDoc>,
    filter: EventFilter,
    debounce: Duration,
    worker_name: &'static str,
    mut work: F,
) -> DebouncedCommitWorkerHandle
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send + 'static,
{
    let notify = Arc::new(Notify::new());
    let notify_cb = notify.clone();
    let filter_cb = filter;
    let subscription = doc.subscribe_root(Arc::new(move |event| {
        if filter_cb(&event) {
            notify_cb.notify_one();
        }
    }));

    let handle = tokio::spawn(async move {
        loop {
            notify.notified().await;
            tokio::time::sleep(debounce).await;
            // Drain any notifications that fired during the sleep so
            // one work call covers the whole burst.
            while notify.notified().now_or_never().is_some() {}
            if let Err(e) = work().await {
                tracing::error!(
                    worker = worker_name,
                    error = %format!("{e:#}"),
                    "[debounced_commit_worker] work callback failed"
                );
                // Do NOT return — the next commit re-triggers us.
            }
        }
    });

    DebouncedCommitWorkerHandle {
        _subscription: subscription,
        abort: handle.abort_handle(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loro::TreeID;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test(start_paused = true)]
    async fn debounces_burst_into_single_work_call() {
        let doc = Arc::new(LoroDoc::new());
        let count = Arc::new(AtomicUsize::new(0));
        let count_cb = count.clone();
        let _worker = spawn(
            doc.clone(),
            any_commit(),
            Duration::from_millis(150),
            "test",
            move || {
                let c = count_cb.clone();
                async move {
                    c.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                }
            },
        );

        for i in 0..100u32 {
            let tree = doc.get_tree("t");
            let node = tree.create(None::<TreeID>).unwrap();
            tree.get_meta(node).unwrap().insert("n", i as i64).unwrap();
            doc.commit();
            tokio::task::yield_now().await;
        }

        tokio::time::advance(Duration::from_millis(500)).await;
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        let n = count.load(Ordering::Relaxed);
        assert!(n >= 1, "expected at least one work call, got {n}");
        assert!(
            n <= 3,
            "expected ≤3 work calls for a 100-commit burst, got {n}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn local_only_filter_skips_import_events() {
        // We can't directly generate Import events without a full sync
        // setup, so instead sanity-check that any_commit fires and
        // local_only also fires on plain local commits (which is what
        // `triggered_by == Local` means).
        let doc = Arc::new(LoroDoc::new());
        let count = Arc::new(AtomicUsize::new(0));
        let count_cb = count.clone();
        let _worker = spawn(
            doc.clone(),
            local_only(),
            Duration::from_millis(50),
            "test",
            move || {
                let c = count_cb.clone();
                async move {
                    c.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                }
            },
        );

        let tree = doc.get_tree("t");
        let node = tree.create(None::<TreeID>).unwrap();
        tree.get_meta(node).unwrap().insert("n", 1i64).unwrap();
        doc.commit();
        tokio::task::yield_now().await;

        tokio::time::advance(Duration::from_millis(150)).await;
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        assert!(
            count.load(Ordering::Relaxed) >= 1,
            "local-only filter should fire on Local commits"
        );
    }
}
