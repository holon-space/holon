//! Let-it-die supervision for a derived-data stream.
//!
//! # Why supervision lives here and not in the combinator
//!
//! [`home_by`](super::home_by) latches terminally on an authority error: it
//! yields the `Err` and then ends. That is deliberate — a combinator that keeps
//! folding on an accumulator it could not update produces silently wrong
//! derived state, which is worse than none. The consequence is that recovery
//! cannot be a component concern: the component is *dead*, by design.
//!
//! Recovery is therefore a composition-root concern. The supervisor holds the
//! stream *factory*, not the stream: on death it drops the dead stream (and
//! with it the accumulator, which lives inside), builds a fresh one from the
//! same container-resolved handles, and tells the consumer to drop its derived
//! state. The fresh stream's first input is the feed's `MapDiff::Replace`
//! snapshot, so re-seeding is not a second mechanism — it is the boot path.
//!
//! # One code path, exercised every cold start
//!
//! [`Supervised::Reset`] is emitted before *every* stream, including the first.
//! A consumer that handles it correctly at boot handles it correctly at
//! restart, and there is no recovery-only branch that only runs after a rare
//! failure.
//!
//! # Bounded restarts
//!
//! An authority that is down stays down, and an unbounded restart loop would
//! turn that into a silent hot spin. After
//! [`MAX_RESTARTS_IN_WINDOW`] restarts inside [`RESTART_WINDOW`] the supervisor
//! stops and calls the give-up seam, which is how a persistent failure reaches
//! degraded-mode disclosure instead of dying quietly.

use std::collections::VecDeque;
use std::time::Duration;
use std::time::Instant;

use anyhow::Error;
use futures::stream::Stream;
use futures::stream::StreamExt as _;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;

/// Restarts tolerated inside [`RESTART_WINDOW`] before escalating.
pub const MAX_RESTARTS_IN_WINDOW: u32 = 3;

/// Rolling window the restart count is measured over.
pub const RESTART_WINDOW: Duration = Duration::from_secs(60);

/// What a supervised consumer receives.
///
/// `Reset` is not an error signal — it is the start-of-seeding marker, emitted
/// at boot and at every restart alike.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Supervised<D> {
    /// Drop all derived state; a complete fresh seeding follows.
    Reset,
    Diff(D),
}

/// Drive a fallible derived-data stream, restarting it when it dies.
///
/// `make_stream` is called once per incarnation; each call must produce a
/// stream that seeds itself completely (for `LiveData` combinators this is
/// automatic — a fresh subscription opens with `MapDiff::Replace`).
///
/// Returns when the consumer hangs up, when a stream ends without an error
/// (the source is gone — shutdown, not failure), or when the restart budget is
/// spent.
pub async fn run_supervised<D, S, F, G>(
    component: &'static str,
    mut make_stream: F,
    tx: UnboundedSender<Supervised<D>>,
    on_gave_up: G,
) where
    F: FnMut() -> S,
    S: Stream<Item = anyhow::Result<D>>,
    G: Fn(&'static str, u32, &Error),
{
    let mut restarts: VecDeque<Instant> = VecDeque::new();
    loop {
        if tx.send(Supervised::Reset).is_err() {
            return;
        }
        let mut stream = std::pin::pin!(make_stream());
        let mut death: Option<Error> = None;
        while let Some(item) = stream.next().await {
            match item {
                Ok(diff) => {
                    if tx.send(Supervised::Diff(diff)).is_err() {
                        return;
                    }
                }
                Err(e) => {
                    death = Some(e);
                    break;
                }
            }
        }
        let Some(err) = death else {
            tracing::info!(
                "[supervisor:{component}] stream ended without error — source gone, not \
                 restarting"
            );
            return;
        };

        let now = Instant::now();
        while restarts
            .front()
            .is_some_and(|t| now.duration_since(*t) > RESTART_WINDOW)
        {
            restarts.pop_front();
        }
        restarts.push_back(now);
        let count = restarts.len() as u32;
        if count > MAX_RESTARTS_IN_WINDOW {
            tracing::error!(
                "[supervisor:{component}] DIED {count} times within {:?} — GIVING UP. Last \
                 error: {err:#}. Derived state is now permanently stale for this process; \
                 escalating to degraded mode.",
                RESTART_WINDOW
            );
            on_gave_up(component, count, &err);
            return;
        }
        tracing::error!(
            "[supervisor:{component}] stream DIED: {err:#} — restarting (restart {count}/{} in \
             the last {:?}); consumer state is dropped and re-seeded from the source snapshot",
            MAX_RESTARTS_IN_WINDOW,
            RESTART_WINDOW
        );
    }
}

/// Spawn [`run_supervised`] on the current runtime, handing back the consumer
/// end.
pub fn spawn_supervised<D, S, F, G>(
    component: &'static str,
    make_stream: F,
    on_gave_up: G,
) -> UnboundedReceiver<Supervised<D>>
where
    D: Send + 'static,
    S: Stream<Item = anyhow::Result<D>> + Send,
    F: FnMut() -> S + Send + 'static,
    G: Fn(&'static str, u32, &Error) + Send + 'static,
{
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        run_supervised(component, make_stream, tx, on_gave_up).await;
    });
    rx
}
