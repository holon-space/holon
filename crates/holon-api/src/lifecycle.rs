//! Orderly session shutdown: stop what a boot spawned, then close the store.
//!
//! A session's background work (the org-writeback supervisor, the file-sync
//! controller, the UI watchers, the rule watchers) outlives every handle the
//! caller holds — the tasks own their own state, so dropping an `Arc` does not
//! stop them. Closing the storage actor underneath them therefore turns each
//! surviving task into a stream of failed reads, and the supervised ones burn
//! their restart budget and escalate to permanently degraded.
//!
//! [`SessionShutdown`] is the one seam that orders those two events. Every
//! session-scoped task is spawned through it and observes its
//! [`SessionShutdown::cancelled`] signal; [`SessionShutdown::shutdown`] cancels
//! and joins them all within a bound the caller states, and reports the tasks
//! that did not stop by name.

use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// The bound a session shutdown gives its tasks. Long enough for a task parked
/// on a CDC stream to observe the cancellation and unwind, short enough that a
/// wedged task is reported rather than waited on.
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Tasks that were still running when the shutdown bound expired. Reported —
/// never detached silently — because a task outliving the shutdown is exactly
/// the defect this type exists to prevent.
#[derive(Debug, thiserror::Error)]
#[error(
    "session shutdown timed out after {}ms — still running: [{}]. These tasks outlive the \
     session and will fail against the closed store.",
    .timeout.as_millis(),
    .still_running.join(", ")
)]
pub struct ShutdownTimedOut {
    pub timeout: Duration,
    pub still_running: Vec<String>,
}

/// The session's background-task registry and its cancellation signal.
///
/// Registered once per session (DI-root scoped) and resolved by every wiring
/// that spawns long-lived work.
pub struct SessionShutdown {
    cancel: CancellationToken,
    tasks: Mutex<Vec<(String, JoinHandle<()>)>>,
}

impl Default for SessionShutdown {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionShutdown {
    pub fn new() -> Self {
        Self {
            cancel: CancellationToken::new(),
            tasks: Mutex::new(Vec::new()),
        }
    }

    /// Spawn session-scoped background work under this shutdown.
    ///
    /// `name` identifies the task in a timeout report, so it must name the
    /// component a reader would go looking for (`"org-writeback"`,
    /// `"file-sync-controller"`), not the call site.
    ///
    /// The future is expected to observe [`Self::cancelled`]; work that cannot
    /// (a bounded one-shot) is still registered so the shutdown joins it.
    pub fn spawn<F>(&self, name: impl Into<String>, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let name = name.into();
        assert!(
            !self.cancel.is_cancelled(),
            "[SessionShutdown] '{name}' was spawned after shutdown — the task would never be \
             joined and would outlive the store it reads"
        );
        let handle = tokio::spawn(future);
        self.tasks
            .lock()
            .expect("SessionShutdown task registry poisoned")
            .push((name, handle));
    }

    /// The tasks registered and not yet joined, by name.
    ///
    /// Exists so a test can assert that a family it cares about is actually
    /// registered: "shutdown reported no stragglers" says nothing about a
    /// watcher that was never handed to this type in the first place.
    pub fn registered(&self) -> Vec<String> {
        self.tasks
            .lock()
            .expect("SessionShutdown task registry poisoned")
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Resolves when the session is shutting down. Long-lived loops select on
    /// this alongside their input stream.
    pub fn cancelled(&self) -> tokio_util::sync::WaitForCancellationFutureOwned {
        self.cancel.clone().cancelled_owned()
    }

    /// A clonable handle to the cancellation signal, for work that outlives the
    /// borrow (a task moved into `tokio::spawn`).
    pub fn token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Cancel every registered task and join it within `timeout`.
    ///
    /// Call this BEFORE closing the storage actor. Returns the tasks that did
    /// not stop; a caller that ignores the `Err` re-creates the defect.
    pub async fn shutdown(&self, timeout: Duration) -> Result<(), ShutdownTimedOut> {
        self.cancel.cancel();

        let tasks = std::mem::take(
            &mut *self
                .tasks
                .lock()
                .expect("SessionShutdown task registry poisoned"),
        );

        let deadline = tokio::time::Instant::now() + timeout;
        let mut still_running = Vec::new();
        for (name, handle) in tasks {
            match tokio::time::timeout_at(deadline, handle).await {
                // A panicked task already surfaced its own panic; joining it is
                // still a stop, which is what this bound is about.
                Ok(_joined) => {}
                Err(_elapsed) => still_running.push(name),
            }
        }

        if still_running.is_empty() {
            Ok(())
        } else {
            Err(ShutdownTimedOut {
                timeout,
                still_running,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;

    use super::*;

    #[tokio::test]
    async fn shutdown_stops_a_task_parked_on_its_cancellation() {
        let shutdown = SessionShutdown::new();
        let stopped = Arc::new(AtomicBool::new(false));

        let flag = stopped.clone();
        let cancelled = shutdown.cancelled();
        shutdown.spawn("parked-forever", async move {
            cancelled.await;
            flag.store(true, Ordering::SeqCst);
        });

        shutdown
            .shutdown(DEFAULT_SHUTDOWN_TIMEOUT)
            .await
            .expect("a task that observes cancellation must stop within the bound");
        assert!(stopped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_task_that_ignores_cancellation_is_reported_by_name() {
        let shutdown = SessionShutdown::new();
        shutdown.spawn("deaf-loop", async {
            std::future::pending::<()>().await;
        });

        let err = shutdown
            .shutdown(Duration::from_millis(50))
            .await
            .expect_err("a task that never observes cancellation must NOT be detached silently");
        assert_eq!(err.still_running, vec!["deaf-loop".to_string()]);
    }
}
