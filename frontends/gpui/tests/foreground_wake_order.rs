//! Which foreground task runs first when data reaches a view bridge and a
//! watermark relay is woken right after it.
//!
//! A bridge is a foreground task streaming a `MutableVec` into a view; the
//! relay is a foreground task that is told "data through S is applied" and
//! must not report S before the bridge has taken S. Both are woken from a
//! non-UI thread, the way a tokio watcher wakes them.
//!
//! Two schedules:
//! - queued: the bridge is idle when S arrives, so its wake enqueues it ahead
//!   of the relay;
//! - mid-poll: the bridge is being polled when S arrives, so its wake only
//!   flags it and it is enqueued when that poll ends, behind the relay.
//!
//! Two relay designs: check at once (`Snapshot`), or yield once first
//! (`YieldOnce`).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::task::Context;
use std::task::Poll;

use futures::StreamExt;
use futures::channel::mpsc::UnboundedSender;
use futures::channel::mpsc::unbounded;
use futures_signals::signal_vec::MutableVec;
use futures_signals::signal_vec::SignalVecExt;
use futures_signals::signal_vec::VecDiff;
use gpui::HeadlessAppContext;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Schedule {
    Queued,
    MidPoll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Relay {
    Snapshot,
    YieldOnce,
}

struct YieldOnce(bool);

impl Future for YieldOnce {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            return Poll::Ready(());
        }
        self.0 = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

/// Pushes S into the bridge's vec and wakes the relay with S, from a non-UI
/// thread, in that order: the watcher applies, then reports.
fn deliver(vec: &MutableVec<u64>, relay_tx: &UnboundedSender<u64>, seq: u64) {
    vec.lock_mut().push_cloned(seq);
    relay_tx
        .unbounded_send(seq)
        .expect("the relay task is alive while the probe runs");
}

/// Whether the relay, when it reported S, saw the bridge holding S.
fn relay_saw_bridge_applied(schedule: Schedule, relay: Relay) -> bool {
    const SEQ: u64 = 1;
    let mut app = HeadlessAppContext::new(gpui_platform::current_text_system());
    app.allow_parking();

    let vec = MutableVec::<u64>::new();
    let bridge_applied = Arc::new(AtomicU64::new(0));
    let reports: Arc<Mutex<Vec<bool>>> = Arc::default();
    let (relay_tx, mut relay_rx) = unbounded::<u64>();

    {
        let bridge_applied = bridge_applied.clone();
        let reports = reports.clone();
        app.update(|cx| {
            cx.spawn(async move |_| {
                while let Some(seq) = relay_rx.next().await {
                    if relay == Relay::YieldOnce {
                        YieldOnce(false).await;
                    }
                    let applied = bridge_applied.load(Ordering::SeqCst) >= seq;
                    reports.lock().unwrap().push(applied);
                }
            })
            .detach()
        });
    }

    // In the mid-poll schedule the bridge's first Pending poll hands control
    // to the delivering thread and blocks the UI thread until S was delivered.
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let (mut block_in_poll, go_after_idle) = match schedule {
        Schedule::MidPoll => (Some((go_tx, done_rx)), None),
        Schedule::Queued => (None, Some(go_tx)),
    };
    {
        let bridge_applied = bridge_applied.clone();
        let mut stream = vec.signal_vec_cloned().to_stream();
        app.update(|cx| {
            cx.spawn(async move |_| {
                loop {
                    let next = futures::future::poll_fn(|cx| match stream.poll_next_unpin(cx) {
                        Poll::Ready(diff) => Poll::Ready(diff),
                        Poll::Pending => {
                            if let Some((go, done)) = block_in_poll.take() {
                                go.send(()).expect("the delivering thread waits for go");
                                done.recv().expect("the delivering thread reports done");
                            }
                            Poll::Pending
                        }
                    })
                    .await;
                    match next {
                        Some(VecDiff::Push { value }) => {
                            bridge_applied.store(value, Ordering::SeqCst)
                        }
                        Some(_) => {}
                        None => break,
                    }
                }
            })
            .detach()
        });
    }

    let deliverer = {
        let vec = vec.clone();
        let relay_tx = relay_tx.clone();
        std::thread::spawn(move || {
            go_rx.recv().expect("the probe sends go");
            deliver(&vec, &relay_tx, SEQ);
            if schedule == Schedule::MidPoll {
                done_tx.send(()).expect("the bridge waits for done");
            }
        })
    };
    app.run_until_parked();
    if let Some(go) = go_after_idle {
        assert_eq!(
            bridge_applied.load(Ordering::SeqCst),
            0,
            "the queued schedule needs an idle bridge before S arrives"
        );
        go.send(()).expect("the delivering thread waits for go");
    }
    deliverer.join().expect("the delivering thread finished");
    app.run_until_parked();

    assert_eq!(
        bridge_applied.load(Ordering::SeqCst),
        SEQ,
        "the bridge took S once the queue drained"
    );
    let reports = reports.lock().unwrap().clone();
    assert_eq!(reports.len(), 1, "the relay reported S exactly once");
    reports[0]
}

#[test]
fn queued_bridge_runs_before_a_relay_woken_after_it() {
    assert!(relay_saw_bridge_applied(Schedule::Queued, Relay::Snapshot));
    assert!(relay_saw_bridge_applied(Schedule::Queued, Relay::YieldOnce));
}

#[test]
fn mid_poll_bridge_runs_after_a_relay_woken_after_it() {
    assert!(
        !relay_saw_bridge_applied(Schedule::MidPoll, Relay::Snapshot),
        "a relay that checks at once reports S before a bridge that was mid-poll when S arrived"
    );
}

#[test]
fn a_relay_that_yields_once_runs_after_a_mid_poll_bridge() {
    assert!(relay_saw_bridge_applied(
        Schedule::MidPoll,
        Relay::YieldOnce
    ));
}
