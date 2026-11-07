//! The UI watermark over random schedules of routing, applying and sampling.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use anyhow::Result;
use holon_api::live_data::DeliveryCursor;
use holon_api::live_data::OwedChild;
use holon_api::live_data::reporting_cursor;
use holon_api::live_data::reporting_to;
use holon_frontend::render_epoch::EngineSeq;
use holon_frontend::render_epoch::UiSubscription;
use holon_frontend::render_epoch::UiWatermark;
use proptest::prelude::*;

/// An engine that routes every change it emits at once.
#[derive(Default)]
struct RoutedEngine(AtomicU64);

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

    async fn delivery_owed(&self) -> Result<Vec<String>> {
        anyhow::bail!("RoutedEngine does not track delivery")
    }

    fn routed_through(&self) -> Result<u64> {
        Ok(self.0.load(Ordering::Acquire))
    }

    fn emitted_through(&self) -> Result<u64> {
        Ok(self.0.load(Ordering::Acquire))
    }

    fn operations_in_flight(&self) -> usize {
        0
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
    /// Subscription `k` shows every batch routed to it and its live children.
    Apply(usize),
    /// Subscription `k` starts a render with its own owed child cursor.
    Child(usize),
    /// A live child of subscription `k` ends unapplied, like a superseded
    /// render: its batches are never shown.
    End(usize, proptest::sample::Index),
    /// The relay samples the watermark and publishes it.
    Publish,
}

fn event(subscriptions: usize) -> impl Strategy<Value = Event> {
    prop_oneof![
        (
            0..subscriptions,
            proptest::option::of(any::<proptest::sample::Index>())
        )
            .prop_map(|(k, child)| Event::Route(k, child)),
        (0..subscriptions).prop_map(Event::Apply),
        (0..subscriptions).prop_map(Event::Child),
        (0..subscriptions, any::<proptest::sample::Index>()).prop_map(|(k, i)| Event::End(k, i)),
        Just(Event::Publish),
    ]
}

/// One owed child of a subscription and the seqs routed to it.
struct Child {
    cursor: OwedChild,
    routed: Vec<u64>,
}

fn schedule() -> impl Strategy<Value = (usize, Vec<Event>)> {
    (1usize..4).prop_flat_map(|n| (Just(n), proptest::collection::vec(event(n), 0..40)))
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
    /// A published watermark never claims a seq whose rows a subscription has
    /// not shown, and every publish is monotonic. Once every subscription
    /// showed what its live cursors were routed, nothing is owed: an ended
    /// child holds no one.
    #[test]
    fn the_published_watermark_never_runs_ahead_of_a_view((n, events) in schedule()) {
        let engine = Arc::new(RoutedEngine::default());
        let watermark = UiWatermark::new(Some(engine.clone()));
        let subscriptions: Vec<UiSubscription> =
            (0..n).map(|k| watermark.register(format!("view-{k}"))).collect();
        subscriptions.iter().for_each(take_cursor);
        let mut routed_to: Vec<Vec<u64>> = vec![Vec::new(); n];
        let mut children: Vec<Vec<Child>> = (0..n).map(|_| Vec::new()).collect();
        let mut shown = vec![0u64; n];
        // The router keeps an ended render's cursor until it finds the
        // subscription closed, which a quiet store may never make it do.
        let mut router_held: Vec<Arc<DeliveryCursor>> = Vec::new();
        let mut seq = 0u64;
        for event in events {
            match event {
                Event::Route(k, child) => {
                    seq += 1;
                    for (j, subscription) in subscriptions.iter().enumerate() {
                        if j != k {
                            subscription.cursor().skipped(seq);
                        }
                    }
                    match child.filter(|_| !children[k].is_empty()) {
                        Some(i) => {
                            let len = children[k].len();
                            let child = &mut children[k][i.index(len)];
                            child.cursor.cursor().routed(seq);
                            child.routed.push(seq);
                        }
                        None => {
                            subscriptions[k].cursor().routed(seq);
                            routed_to[k].push(seq);
                        }
                    }
                    engine.0.store(seq, Ordering::Release);
                }
                Event::Apply(k) => {
                    let through = subscriptions[k].cursor().sent_with_children();
                    subscriptions[k].applied(through);
                    shown[k] = through;
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
                    for k in 0..n {
                        let unshown = routed_to[k]
                            .iter()
                            .chain(children[k].iter().flat_map(|child| &child.routed))
                            .find(|s| **s > shown[k] && **s <= published);
                        prop_assert!(
                            unshown.is_none(),
                            "published {published}, but view-{k} shows through {} and was routed {unshown:?}",
                            shown[k]
                        );
                    }
                }
            }
        }
        for (k, subscription) in subscriptions.iter().enumerate() {
            let live = routed_to[k]
                .iter()
                .chain(children[k].iter().flat_map(|child| &child.routed))
                .max()
                .copied()
                .unwrap_or(0);
            subscription.applied(live.max(shown[k]));
        }
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
