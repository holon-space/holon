//! Which engine seq the UI shows: the watermark the UI subscriptions have
//! applied, and the epoch a frontend stamps on each committed frame.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

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
/// below the first batch it owes after that.
pub struct UiWatermark {
    engine: Option<Arc<dyn holon_api::QueryEngine>>,
    subscriptions: Mutex<(u64, BTreeMap<u64, Entry>)>,
    checks: Mutex<Checks>,
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

    /// The seq the UI could show now. Reads the routed seq before the
    /// subscriptions, so a batch routed in between is still counted as owed.
    pub fn sample(&self) -> anyhow::Result<EngineSeq> {
        let EngineSeq(routed) = self.routed()?;
        let published = self.published.load(Ordering::Acquire);
        let (_, entries) = &*self.subscriptions.lock().unwrap();
        let floor = entries
            .values()
            .map(|entry| match entry.primed {
                true => entry.cursor.owed_floor().unwrap_or(u64::MAX),
                false => published,
            })
            .fold(routed, u64::min);
        Ok(EngineSeq(floor))
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

    /// One line per subscription that holds the watermark: its key and why.
    pub fn owed(&self) -> Vec<String> {
        let mut owed: Vec<String> = {
            let (_, entries) = &*self.subscriptions.lock().unwrap();
            entries
                .values()
                .filter_map(|entry| match (entry.primed, entry.cursor.owed_floor()) {
                    (false, _) => Some(format!("{}: no event applied yet", entry.key)),
                    (true, Some(floor)) => Some(format!("{}: applied through {floor}", entry.key)),
                    (true, None) => None,
                })
                .collect()
        };
        // Cloned out of the lock: a check takes its consumer's locks.
        let checks: Vec<(String, Pending)> =
            self.checks.lock().unwrap().1.values().cloned().collect();
        owed.extend(
            checks
                .into_iter()
                .filter(|(_, pending)| pending())
                .map(|(key, _)| format!("{key}: not applied yet")),
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
