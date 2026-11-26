//! Which engine seq the UI shows: the watermark the UI subscriptions have
//! applied, and the epoch a frontend stamps on each committed frame.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;
use std::task::Wake;
use std::task::Waker;

use futures::Stream;
use holon_api::delivery_ledger::DeliveryLedger;
use holon_api::delivery_ledger::DemuxId;
use holon_api::delivery_ledger::Owed;
use holon_api::delivery_ledger::Seq;
use holon_api::delivery_ledger::ViewKey;
use holon_api::live_data::DeliveryCursor;

/// A frontend's count of committed frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FrameNo(pub u64);

/// A CDC seq of the engine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EngineSeq(pub u64);

/// One committed frame and the engine seq its content reflects: every change
/// up to `reflects` is applied by each UI subscription the frame was built
/// from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderEpoch {
    pub frame: FrameNo,
    pub reflects: EngineSeq,
}

struct Entry {
    key: String,
    cursor: Arc<DeliveryCursor>,
    primed: bool,
}

type Pending = Arc<dyn Fn() -> bool + Send + Sync>;
type Checks = (u64, BTreeMap<u64, (String, Pending)>);

/// The engine seq the UI subscriptions of one session have applied.
///
/// A subscription holds the watermark until it applied its first event, and
/// below the first batch it owes after that. A hand-off holds it where it is
/// while a consumer has not yet applied what its subscription handed on.
pub struct UiWatermark {
    engine: Option<Arc<dyn holon_api::QueryEngine>>,
    subscriptions: Mutex<(u64, BTreeMap<u64, Entry>)>,
    checks: Mutex<Checks>,
    handoffs: Mutex<Checks>,
    published: AtomicU64,
    changed: tokio::sync::watch::Sender<u64>,
}

impl UiWatermark {
    /// `engine` is `None` in a session without CDC: then nothing is routed and
    /// the watermark stays at 0.
    pub fn new(engine: Option<Arc<dyn holon_api::QueryEngine>>) -> Arc<Self> {
        Arc::new(Self {
            engine,
            subscriptions: Mutex::new((0, BTreeMap::new())),
            checks: Mutex::new((0, BTreeMap::new())),
            handoffs: Mutex::new((0, BTreeMap::new())),
            published: AtomicU64::new(0),
            changed: tokio::sync::watch::channel(0).0,
        })
    }

    /// A UI subscription whose watcher subscribes under
    /// `reporting_to(subscription.cursor())`.
    pub fn register(self: &Arc<Self>, key: impl Into<String>) -> UiSubscription {
        let cursor = Arc::new(DeliveryCursor::default());
        let mut guard = self.subscriptions.lock().unwrap();
        let (next, entries) = &mut *guard;
        let id = *next;
        *next += 1;
        entries.insert(
            id,
            Entry {
                key: key.into(),
                cursor: cursor.clone(),
                primed: false,
            },
        );
        drop(guard);
        UiSubscription {
            watermark: self.clone(),
            id,
            cursor,
        }
    }

    /// A consumer owed while `pending()` holds, for UI work no engine seq
    /// names (a batch with no new seq, a focus change): it never lowers
    /// [`Self::sample`].
    pub fn register_check(
        self: &Arc<Self>,
        key: impl Into<String>,
        pending: impl Fn() -> bool + Send + Sync + 'static,
    ) -> UiCheck {
        let mut guard = self.checks.lock().unwrap();
        let (next, checks) = &mut *guard;
        let id = *next;
        *next += 1;
        checks.insert(id, (key.into(), Arc::new(pending)));
        UiCheck {
            watermark: self.clone(),
            id,
        }
    }

    /// A consumer that applies, on its own executor, what a UI subscription
    /// handed on: while `pending()` holds, [`Self::sample`] stays at the
    /// published seq. The consumer calls [`Self::poke`] after each apply.
    pub fn register_handoff(
        self: &Arc<Self>,
        key: impl Into<String>,
        pending: impl Fn() -> bool + Send + Sync + 'static,
    ) -> UiHandoff {
        let mut guard = self.handoffs.lock().unwrap();
        let (next, handoffs) = &mut *guard;
        let id = *next;
        *next += 1;
        handoffs.insert(id, (key.into(), Arc::new(pending)));
        UiHandoff {
            watermark: self.clone(),
            id,
        }
    }

    /// The hand-offs whose consumer has not applied what it was handed.
    fn pending_handoffs(&self) -> Vec<String> {
        // Cloned out of the lock: a hand-off takes its consumer's locks.
        let handoffs: Vec<(String, Pending)> =
            self.handoffs.lock().unwrap().1.values().cloned().collect();
        handoffs
            .into_iter()
            .filter(|(_, pending)| pending())
            .map(|(key, _)| key)
            .collect()
    }

    /// The subscriptions' deliveries, with no demux fed.
    fn ledger(&self) -> DeliveryLedger {
        let mut ledger = DeliveryLedger::default();
        let (_, entries) = &*self.subscriptions.lock().unwrap();
        for (id, entry) in entries {
            let key = ViewKey {
                name: entry.key.clone(),
                instance: *id,
            };
            entry.cursor.feed(&mut ledger, key, entry.primed);
        }
        ledger
    }

    /// The seq the UI could show now. Reads the routed seq before the
    /// subscriptions, so a batch routed in between is still counted as owed.
    pub fn sample(&self) -> anyhow::Result<EngineSeq> {
        let EngineSeq(routed) = self.routed()?;
        let published = self.published.load(Ordering::Acquire);
        let old_floor = || {
            let (_, entries) = &*self.subscriptions.lock().unwrap();
            entries
                .values()
                .map(|entry| match entry.primed {
                    true => {
                        let applied = entry.cursor.applied();
                        if applied < entry.cursor.sent_with_children() { applied } else { u64::MAX }
                    }
                    false => published,
                })
                .fold(routed, u64::min)
        };
        let before = old_floor();
        let mut ledger = self.ledger();
        ledger.routed(DemuxId(0), Seq(routed));
        let Seq(floor) = ledger.complete_through(Seq(published));
        let failing = ledger.owed().iter().any(|o| matches!(o, Owed::RequeryFailed { .. }));
        if !failing && before == old_floor() {
            assert_eq!(floor, before, "DIFFERENTIAL ui floor: ledger left, old right");
            eprintln!("DIFFERENTIAL ui floor agrees");
        }
        // Read after the subscriptions: a subscription hands on before it
        // reports applied, so a hand-off for any seq counted above is seen.
        match self.pending_handoffs().is_empty() {
            true => Ok(EngineSeq(floor)),
            false => Ok(EngineSeq(floor.min(published))),
        }
    }

    /// The seq through which the engine routed every change to its watchers.
    pub fn routed(&self) -> anyhow::Result<EngineSeq> {
        Ok(EngineSeq(match &self.engine {
            Some(engine) => engine.routed_through()?,
            None => 0,
        }))
    }

    /// The seq of the last change the engine emitted: what a settled UI
    /// reflects.
    pub fn emitted(&self) -> anyhow::Result<EngineSeq> {
        Ok(EngineSeq(match &self.engine {
            Some(engine) => engine.emitted_through()?,
            None => 0,
        }))
    }

    /// The engine's admitted operations that have not finished: a settled UI
    /// waits for their writes.
    pub fn operations_in_flight(&self) -> usize {
        self.engine
            .as_ref()
            .map_or(0, |engine| engine.operations_in_flight())
    }

    /// Advance the published watermark to `seq`; true when it moved.
    pub fn publish(&self, seq: EngineSeq) -> bool {
        self.published.fetch_max(seq.0, Ordering::AcqRel) < seq.0
    }

    pub fn current(&self) -> EngineSeq {
        EngineSeq(self.published.load(Ordering::Acquire))
    }

    /// Every delivery a subscription still owes.
    pub fn owed_deliveries(&self) -> Vec<Owed> {
        let old_owed = || {
            let (_, entries) = &*self.subscriptions.lock().unwrap();
            let mut old: Vec<(String, Option<u64>)> = entries
                .values()
                .filter_map(|entry| {
                    let applied = entry.cursor.applied();
                    match (entry.primed, applied < entry.cursor.sent_with_children()) {
                        (false, _) => Some((entry.key.clone(), None)),
                        (true, true) => Some((entry.key.clone(), Some(applied))),
                        (true, false) => None,
                    }
                })
                .collect();
            old.sort();
            old
        };
        let before = old_owed();
        let owed = self.ledger().owed();
        let mut new: Vec<(String, Option<u64>)> = owed
            .iter()
            .filter_map(|o| match o {
                Owed::NotPrimed { view } => Some((view.clone(), None)),
                Owed::Applied { view, applied, .. } => Some((view.clone(), Some(applied.0))),
                _ => None,
            })
            .collect();
        new.sort();
        let failing = owed.iter().any(|o| matches!(o, Owed::RequeryFailed { .. }));
        if !failing && before == old_owed() {
            assert_eq!(new, before, "DIFFERENTIAL ui owed: ledger left, old right");
            eprintln!("DIFFERENTIAL ui owed agrees");
        }
        owed
    }

    /// One line per delivery, check or hand-off that holds the watermark.
    pub fn owed(&self) -> Vec<String> {
        let mut owed: Vec<String> = self
            .owed_deliveries()
            .iter()
            .map(ToString::to_string)
            .collect();
        // Cloned out of the lock: a check takes its consumer's locks.
        let checks: Vec<(String, Pending)> =
            self.checks.lock().unwrap().1.values().cloned().collect();
        owed.extend(
            checks
                .into_iter()
                .filter(|(_, pending)| pending())
                .map(|(key, _)| format!("{key}: not applied yet")),
        );
        owed.extend(
            self.pending_handoffs()
                .into_iter()
                .map(|key| format!("{key}: handed on, not applied yet")),
        );
        owed
    }

    /// Resolves after the next change of a subscription's applied state, or
    /// after [`Self::poke`].
    pub fn changed(&self) -> impl std::future::Future<Output = ()> + Send + 'static {
        let mut rx = self.changed.subscribe();
        async move {
            rx.changed()
                .await
                .expect("the watermark outlives its change receivers");
        }
    }

    /// Ask for a new sample: the routed seq can advance with no UI event.
    pub fn poke(&self) {
        self.changed.send_modify(|n| *n += 1);
    }
}

/// One registered UI subscription; dropping it unregisters.
pub struct UiSubscription {
    watermark: Arc<UiWatermark>,
    id: u64,
    cursor: Arc<DeliveryCursor>,
}

impl UiSubscription {
    pub fn cursor(&self) -> &Arc<DeliveryCursor> {
        &self.cursor
    }

    /// The consumer showed every batch through `seq`.
    pub fn applied(&self, seq: u64) {
        let mut guard = self.watermark.subscriptions.lock().unwrap();
        let entry = guard.1.get_mut(&self.id).expect("registered until drop");
        assert!(
            self.cursor.is_taken(),
            "UI subscription {} has no delivery cursor: its watcher subscribed outside \
             reporting_to(cursor)",
            entry.key
        );
        self.cursor.applied_through(seq);
        entry.primed = true;
        drop(guard);
        self.watermark.poke();
    }
}

impl Drop for UiSubscription {
    fn drop(&mut self) {
        self.watermark
            .subscriptions
            .lock()
            .unwrap()
            .1
            .remove(&self.id);
        self.watermark.poke();
    }
}

/// One registered check; dropping it unregisters.
pub struct UiCheck {
    watermark: Arc<UiWatermark>,
    id: u64,
}

impl Drop for UiCheck {
    fn drop(&mut self) {
        self.watermark.checks.lock().unwrap().1.remove(&self.id);
        self.watermark.poke();
    }
}

/// One registered hand-off; dropping it unregisters.
pub struct UiHandoff {
    watermark: Arc<UiWatermark>,
    id: u64,
}

impl Drop for UiHandoff {
    fn drop(&mut self) {
        self.watermark.handoffs.lock().unwrap().1.remove(&self.id);
        self.watermark.poke();
    }
}

#[derive(Default)]
struct HandoffFlags {
    /// The producer woke the consumer, or the consumer holds the item it
    /// took last: both end with a poll that finds nothing ready.
    owed: AtomicBool,
    /// The consumer is inside a poll.
    polling: AtomicBool,
}

struct FlagWaker {
    flags: Arc<HandoffFlags>,
    inner: Waker,
}

impl Wake for FlagWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.flags.owed.store(true, Ordering::SeqCst);
        self.inner.wake_by_ref();
    }
}

/// `stream` as a UI consumer reads it on its own executor, one item applied
/// before it polls for the next: the watermark holds from the producer's wake
/// until a poll finds nothing ready. Without a watermark nothing is held.
pub fn hand_off<S: Stream + Unpin>(
    watermark: Option<Arc<UiWatermark>>,
    key: impl Into<String>,
    stream: S,
) -> HandedOff<S> {
    let flags = Arc::new(HandoffFlags {
        owed: AtomicBool::new(true),
        polling: AtomicBool::new(false),
    });
    let registration = watermark.map(|watermark| {
        let pending = flags.clone();
        let handoff = watermark.register_handoff(key, move || {
            pending.owed.load(Ordering::SeqCst) || pending.polling.load(Ordering::SeqCst)
        });
        (watermark, handoff)
    });
    HandedOff {
        stream,
        flags,
        registration,
    }
}

/// A stream read through [`hand_off`]; dropping it unregisters.
pub struct HandedOff<S> {
    stream: S,
    flags: Arc<HandoffFlags>,
    registration: Option<(Arc<UiWatermark>, UiHandoff)>,
}

impl<S: Stream + Unpin> Stream for HandedOff<S> {
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<S::Item>> {
        let flags = self.flags.clone();
        flags.polling.store(true, Ordering::SeqCst);
        flags.owed.store(false, Ordering::SeqCst);
        let waker = Waker::from(Arc::new(FlagWaker {
            flags: flags.clone(),
            inner: cx.waker().clone(),
        }));
        let polled = Pin::new(&mut self.stream).poll_next(&mut Context::from_waker(&waker));
        let took = matches!(polled, Poll::Ready(Some(_)));
        if took {
            flags.owed.store(true, Ordering::SeqCst);
        }
        flags.polling.store(false, Ordering::SeqCst);
        if let (false, Some((watermark, _))) = (took, &self.registration) {
            watermark.poke();
        }
        polled
    }
}
