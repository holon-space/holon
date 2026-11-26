//! The UI watermark over random schedules of routing, applying, handing on
//! to a consumer on another executor, and sampling. Some subscriptions are
//! served by an eager re-query: it reads every change off the CDC broadcast
//! itself, so it routes on its own, and its re-query can fail. Operations
//! are admitted and finish in between.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use anyhow::Result;
use futures::StreamExt;
use futures::channel::mpsc;
use holon_api::delivery_ledger::DeliveryLedger;
use holon_api::delivery_ledger::Owed;
use holon_api::delivery_ledger::Seq;
use holon_api::live_data::DeliveryCursor;
use holon_api::live_data::OwedChild;
use holon_api::live_data::Routers;
use holon_api::live_data::reporting_cursor;
use holon_api::live_data::reporting_to;
use holon_api::query_engine::EngineProgress;
use holon_api::wake_witness::WakeWitness;
use holon_frontend::render_epoch::EngineSeq;
use holon_frontend::render_epoch::HandedOff;
use holon_frontend::render_epoch::UiSubscription;
use holon_frontend::render_epoch::UiWatermark;
use holon_frontend::render_epoch::hand_off;
use proptest::prelude::*;

/// An engine whose demux routes every change it emits at once; an eager
/// re-query routes through its own router. Its progress moves after each
/// change of what it reports, as the `QueryEngine` contract asks.
#[derive(Default)]
struct RoutedEngine {
    emitted: AtomicU64,
    routers: Routers,
    in_flight: AtomicUsize,
    progress: EngineProgress,
}

#[async_trait::async_trait]
impl holon_api::QueryEngine for RoutedEngine {
    async fn lookup_block_path(&self, _: &holon_api::EntityUri) -> Result<String> {
        anyhow::bail!("RoutedEngine has no blocks")
    }

    async fn watch_query(
        &self,
        _: &str,
        _: holon_api::QueryLanguage,
        _: std::collections::HashMap<String, holon_api::Value>,
        _: Option<holon_api::QueryContext>,
        _: holon_api::render_requirements::RenderRequirements,
    ) -> Result<holon_api::EnrichedChangeStream> {
        anyhow::bail!("RoutedEngine does not watch queries")
    }

    async fn delivery_owed(&self) -> Result<Vec<holon_api::delivery_ledger::Owed>> {
        anyhow::bail!("RoutedEngine does not track delivery")
    }

    fn routed_through(&self) -> Result<u64> {
        let mut ledger = DeliveryLedger::default();
        ledger.emitted(Seq(self.emitted.load(Ordering::Acquire)));
        self.routers.feed(&mut ledger);
        Ok(ledger.routed_through().0)
    }

    fn emitted_through(&self) -> Result<u64> {
        Ok(self.emitted.load(Ordering::Acquire))
    }

    fn operations_in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }

    fn progress(&self) -> EngineProgress {
        self.progress.clone()
    }

    async fn search_link_candidates(&self, _: &str) -> Result<Vec<holon_api::LinkCandidate>> {
        anyhow::bail!("RoutedEngine has no blocks")
    }

    async fn block_content_by_id(&self, _: &holon_api::EntityUri) -> Result<Option<String>> {
        anyhow::bail!("RoutedEngine has no blocks")
    }

    async fn block_task_state_by_id(&self, _: &holon_api::EntityUri) -> Result<Option<String>> {
        anyhow::bail!("RoutedEngine has no blocks")
    }
}

#[derive(Clone, Debug)]
enum Event {
    /// The engine emits the next change, with rows for subscription `k` only:
    /// routed to its own cursor (`None`) or to one of its live owed children.
    Route(usize, Option<proptest::sample::Index>),
    /// Subscription `k` applies every batch routed to it and its live
    /// children, and hands the result on to its consumer.
    Apply(usize),
    /// The consumer of subscription `k` shows the item it took last, if any,
    /// and polls for the next one.
    Consume(usize),
    /// Subscription `k` starts a render with its own owed child cursor.
    Child(usize),
    /// A live child of subscription `k` ends unapplied, like a superseded
    /// render: its batches are never shown.
    End(usize, proptest::sample::Index),
    /// The relay samples the watermark and publishes it.
    Publish,
    /// Eager subscription `k` reads every change emitted so far off the
    /// broadcast and routes it to its cursor; with nothing new it does nothing.
    EagerReceive(usize),
    /// Like `EagerReceive`, then its re-query fails with cause `c`.
    EagerFail(usize, usize),
    /// The engine admits an operation.
    Admit,
    /// The engine finishes an admitted operation, if any, with no rows.
    Finish,
}

const CAUSES: [&str; 2] = ["no such table: block", "database is locked"];

fn event(subscriptions: usize) -> impl Strategy<Value = Event> {
    prop_oneof![
        (
            0..subscriptions,
            proptest::option::of(any::<proptest::sample::Index>())
        )
            .prop_map(|(k, child)| Event::Route(k, child)),
        (0..subscriptions).prop_map(Event::Apply),
        (0..subscriptions).prop_map(Event::Consume),
        (0..subscriptions).prop_map(Event::Child),
        (0..subscriptions, any::<proptest::sample::Index>()).prop_map(|(k, i)| Event::End(k, i)),
        Just(Event::Publish),
        (0..subscriptions).prop_map(Event::EagerReceive),
        (0..subscriptions, 0..CAUSES.len()).prop_map(|(k, c)| Event::EagerFail(k, c)),
        Just(Event::Admit),
        Just(Event::Finish),
    ]
}

/// One owed child of a subscription and the seqs routed to it.
struct Child {
    cursor: OwedChild,
    routed: Vec<u64>,
}

/// `n` subscriptions, the last `eager` of them served by an eager re-query.
fn schedule() -> impl Strategy<Value = (usize, usize, Vec<Event>)> {
    (1usize..5).prop_flat_map(|n| {
        (
            Just(n),
            0..=n.min(2),
            any::<bool>(),
            proptest::collection::vec(event(n), 0..40),
        )
            .prop_map(|(n, eager, primed, events)| {
                (n, eager, primed_prefix(n, primed).chain(events).collect())
            })
    })
}

/// When `primed`, every subscription first applies and shows what it has, so
/// no unprimed view holds the watermark back.
fn primed_prefix(n: usize, primed: bool) -> impl Iterator<Item = Event> {
    (0..n)
        .filter(move |_| primed)
        .flat_map(|k| [Event::Apply(k), Event::Consume(k), Event::Consume(k)])
}

/// The consumer side of one subscription: a GPUI bridge, say.
struct Consumer {
    stream: HandedOff<mpsc::UnboundedReceiver<u64>>,
    taken: Option<u64>,
    shown: u64,
}

impl Consumer {
    fn consume(&mut self) -> std::task::Poll<()> {
        if let Some(through) = self.taken.take() {
            self.shown = through;
        }
        let mut cx = std::task::Context::from_waker(futures::task::noop_waker_ref());
        match self.stream.poll_next_unpin(&mut cx) {
            std::task::Poll::Ready(Some(through)) => {
                self.taken = Some(through);
                std::task::Poll::Ready(())
            }
            std::task::Poll::Ready(None) => unreachable!("the test keeps every sender"),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

/// What a settle waiter reads: the sample, the emitted seq, whether anything
/// is owed, and the operations in flight.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Settle {
    sample: EngineSeq,
    emitted: EngineSeq,
    owed: Vec<String>,
    in_flight: usize,
}

impl Settle {
    fn read(watermark: &UiWatermark) -> Self {
        Self {
            sample: watermark.sample().expect("the engine reports its routed seq"),
            emitted: watermark.emitted().expect("the engine reports its emitted seq"),
            owed: watermark.owed(),
            in_flight: watermark.operations_in_flight(),
        }
    }

    fn settled(&self) -> bool {
        self.sample >= self.emitted && self.owed.is_empty() && self.in_flight == 0
    }
}

/// A waiter on [`UiWatermark::changed`] that reads [`Settle`] on each wake.
fn settle_witness(watermark: &Arc<UiWatermark>) -> WakeWitness<Settle> {
    let arm = watermark.clone();
    let read = watermark.clone();
    WakeWitness::new(
        move || Box::pin(arm.changed()),
        move || Settle::read(&read),
    )
}

fn take_cursor(subscription: &UiSubscription) {
    let taken = futures::executor::block_on(reporting_to(subscription.cursor().clone(), async {
        reporting_cursor()
    }));
    assert!(
        taken.is_some(),
        "reporting_to hands its cursor to the subscriber"
    );
}

proptest! {
    /// A published watermark never claims a seq whose rows a subscription's
    /// consumer has not shown, and every publish is monotonic. Once every
    /// subscription applied what its live cursors were routed and every
    /// consumer showed it, nothing is owed: an ended child holds no one.
    ///
    /// An eager subscription owes every emitted change, routed or not; after
    /// a failed re-query it is owed by name with the cause until it applies
    /// again.
    ///
    /// Whenever the settle check holds, a waiter on `changed()` last woke to
    /// the state as it is: every change on the way woke it.
    #[test]
    fn the_published_watermark_never_runs_ahead_of_a_view((n, eager, events) in schedule()) {
        let engine = Arc::new(RoutedEngine::default());
        let watermark = UiWatermark::new(Some(engine.clone()));
        let subscriptions: Vec<UiSubscription> =
            (0..n).map(|k| watermark.register(format!("view-{k}"))).collect();
        subscriptions.iter().for_each(take_cursor);
        let is_eager = |k: usize| k >= n - eager;
        let routers: Vec<Option<Arc<AtomicU64>>> = (0..n)
            .map(|k| is_eager(k).then(|| engine.routers.add(0)))
            .collect();
        let mut failed: Vec<Option<(u64, &str)>> = vec![None; n];
        let (senders, mut consumers): (Vec<_>, Vec<_>) = (0..n)
            .map(|k| {
                let (sender, receiver) = mpsc::unbounded();
                let consumer = Consumer {
                    stream: hand_off(Some(watermark.clone()), format!("consumer-{k}"), receiver),
                    taken: None,
                    shown: 0,
                };
                (sender, consumer)
            })
            .unzip();
        let mut routed_to: Vec<Vec<u64>> = vec![Vec::new(); n];
        let mut children: Vec<Vec<Child>> = (0..n).map(|_| Vec::new()).collect();
        let mut shown = vec![0u64; n];
        // The router keeps an ended render's cursor until it finds the
        // subscription closed, which a quiet store may never make it do.
        let mut router_held: Vec<Arc<DeliveryCursor>> = Vec::new();
        let mut seq = 0u64;
        let witness = settle_witness(&watermark);
        for event in events {
            let step = format!("{event:?}");
            match event {
                Event::Route(k, child) => {
                    seq += 1;
                    for (j, subscription) in subscriptions.iter().enumerate() {
                        if j != k && !is_eager(j) {
                            subscription.cursor().skipped(seq);
                        }
                    }
                    match child.filter(|_| !children[k].is_empty() && !is_eager(k)) {
                        Some(i) => {
                            let len = children[k].len();
                            let child = &mut children[k][i.index(len)];
                            child.cursor.cursor().routed(seq);
                            child.routed.push(seq);
                        }
                        None if is_eager(k) => {}
                        None => {
                            subscriptions[k].cursor().routed(seq);
                            routed_to[k].push(seq);
                        }
                    }
                    engine.emitted.store(seq, Ordering::Release);
                    engine.progress.moved();
                }
                Event::EagerReceive(k) | Event::EagerFail(k, _) => {
                    let Some(router) = routers[k].as_ref().filter(|r| r.load(Ordering::Acquire) < seq) else {
                        continue;
                    };
                    subscriptions[k].cursor().routed(seq);
                    router.fetch_max(seq, Ordering::Release);
                    engine.progress.moved();
                    routed_to[k].push(seq);
                    if let Event::EagerFail(_, c) = event {
                        subscriptions[k].cursor().requery_failed(CAUSES[c].to_string());
                        failed[k] = Some((seq, CAUSES[c]));
                    }
                }
                Event::Apply(k) => {
                    let through = subscriptions[k].cursor().sent_with_children();
                    senders[k].unbounded_send(through).expect("the consumer lives");
                    subscriptions[k].applied(through);
                    shown[k] = through;
                    failed[k] = None;
                }
                Event::Admit => {
                    engine.in_flight.fetch_add(1, Ordering::AcqRel);
                    engine.progress.moved();
                }
                Event::Finish => {
                    if engine.in_flight.load(Ordering::Acquire) > 0 {
                        engine.in_flight.fetch_sub(1, Ordering::AcqRel);
                        engine.progress.moved();
                    }
                }
                Event::Consume(k) => {
                    let _ = consumers[k].consume();
                }
                Event::Child(k) => {
                    let cursor = subscriptions[k].cursor().owed_child();
                    router_held.push(cursor.cursor().clone());
                    children[k].push(Child {
                        cursor,
                        routed: Vec::new(),
                    });
                }
                Event::End(k, i) => {
                    if !children[k].is_empty() {
                        let len = children[k].len();
                        children[k].remove(i.index(len));
                    }
                }
                Event::Publish => {
                    let before = watermark.current();
                    let sample = watermark.sample().expect("the engine reports its routed seq");
                    watermark.publish(sample);
                    let EngineSeq(published) = watermark.current();
                    prop_assert!(watermark.current() >= before);
                    for k in (0..n).filter(|k| is_eager(*k)) {
                        prop_assert!(
                            published <= consumers[k].shown,
                            "published {published}, but eager consumer-{k} shows through {}",
                            consumers[k].shown
                        );
                    }
                    for k in 0..n {
                        let unshown = routed_to[k]
                            .iter()
                            .chain(children[k].iter().flat_map(|child| &child.routed))
                            .find(|s| **s > consumers[k].shown && **s <= published);
                        prop_assert!(
                            unshown.is_none(),
                            "published {published}, but consumer-{k} shows through {} and was routed {unshown:?}",
                            consumers[k].shown
                        );
                    }
                }
            }
            let now = Settle::read(&watermark);
            if now.settled() {
                prop_assert_eq!(
                    witness.seen(),
                    now,
                    "after {}, the settle check holds, but no change on the way woke a waiter on changed()",
                    step
                );
            }
            let owed = watermark.owed_deliveries();
            for (k, failure) in failed.iter().enumerate() {
                let Some((at, cause)) = failure else {
                    continue;
                };
                let named = owed.iter().any(|o| {
                    matches!(o, Owed::RequeryFailed { view, at: owed_at, cause: owed_cause, .. }
                        if *view == format!("view-{k}") && *owed_at == Seq(*at) && owed_cause == cause)
                });
                prop_assert!(named, "view-{k} failed its re-query with {cause:?}, owed {owed:?}");
            }
        }
        for (k, subscription) in subscriptions.iter().enumerate() {
            let live = routed_to[k]
                .iter()
                .chain(children[k].iter().flat_map(|child| &child.routed))
                .max()
                .copied()
                .unwrap_or(0);
            let through = live.max(shown[k]);
            senders[k].unbounded_send(through).expect("the consumer lives");
            subscription.applied(through);
            while consumers[k].consume().is_ready() {}
        }
        prop_assert_eq!(watermark.owed_deliveries(), Vec::<Owed>::new());
        prop_assert_eq!(watermark.owed(), Vec::<String>::new());
    }

    /// A subscription whose watcher never took its cursor panics on its first
    /// apply; one that took it does not.
    #[test]
    fn applying_without_a_taken_cursor_panics(taken in any::<bool>(), through in 0u64..100) {
        let watermark = UiWatermark::new(None);
        let subscription = watermark.register("view");
        if taken {
            take_cursor(&subscription);
        }
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            subscription.applied(through)
        }));
        prop_assert_eq!(outcome.is_err(), !taken);
        std::mem::forget(subscription);
    }
}
