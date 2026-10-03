//! The engine host: the views of one dataflow on a thread of their own, fed
//! per source and released per version of the commit clock.

use std::any::Any;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::panic;
use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;
use std::thread;

use holon_api::Condition;
use holon_api::ConditionBus;
use holon_api::ConditionKind;
use holon_api::EntityUri;
use holon_api::block::SnapshotBlock;
use holon_api::commit_clock::CommitClock;
use holon_api::commit_clock::CommitSource;
use holon_api::commit_clock::Stamp;
use holon_api::condition_bus::VIEW_ENGINE_SUBJECT;
use timely::WorkerConfig;
use timely::communication::Allocator;
use timely::communication::allocator::Thread;
use timely::worker::Worker;

use crate::batch::Multiset;
use crate::batch::add;
use crate::batch::col;
use crate::error::EngineError;
use crate::intern::Interner;
use crate::lower::Dataflow;
use crate::payload::Payload;
use crate::plan::Catalog;
use crate::plan::Checked;
use crate::plan::Col;
use crate::plan::Schema;
use crate::plan::check_all;
use crate::row::Datum;
use crate::row::DynRow;
use crate::row::Id;
use crate::row::Row;
use crate::row::RowKind;
use crate::views::BLOCKS;
use crate::views::FOCUS_ROOTS;
use crate::views::FocusRoot;
use crate::views::HISTORY;
use crate::views::ID;
use crate::views::PARENT;
use crate::views::View;
use crate::views::block_row;
use crate::views::blocks_schema;
use crate::views::catalog;
use crate::views::focus_root_row;
use crate::views::focus_roots_schema;
use crate::views::views;

/// A column of a released row, with ids as block URIs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Field {
    Bool(bool),
    Int(i64),
    Uri(EntityUri),
    Text(Arc<str>),
    Payload(Payload),
}

/// The change of one view from the previous version to the version that
/// holds every commit stamped below `below` and none other.
#[derive(Debug, Clone)]
pub struct ViewBatch {
    pub below: Stamp,
    pub deltas: Vec<(Vec<Field>, isize)>,
}

/// [`CommitSource::Sql`] feeds `focus_roots`; the Loro sources feed `blocks`.
/// The engine thread handles the requests in the order they are posted. Its
/// first error or panic stops it: the views stay at the last released
/// version, every snapshot returns the error, and a feed only feeds the
/// clock, so no watermark waits on a stopped engine. A panic can leave the
/// host state half-updated; only the sticky stop keeps it from being released.
pub struct ViewEngine {
    requests: Sender<Request>,
}

enum Request {
    Snapshot {
        view: View,
        subscriber: Sender<ViewBatch>,
        reply: Sender<Result<ViewBatch, EngineError>>,
    },
    Feed {
        source: CommitSource,
        cover: Stamp,
        rows: Rows,
    },
}

/// A `Replace` holds every row of the source; a fed row not among them is
/// deleted.
enum Rows {
    Delta(Vec<(EntityUri, Option<SnapshotBlock>)>),
    Replace(Vec<SnapshotBlock>),
    FocusDelta(Vec<(i64, Option<FocusRoot>)>),
    FocusReplace(Vec<FocusRoot>),
    Refused(EngineError),
}

impl ViewEngine {
    /// `on_stop` runs on the engine thread with the error that stops it.
    pub fn start(
        clock: Arc<CommitClock>,
        on_stop: impl FnOnce(&EngineError) + Send + 'static,
    ) -> ViewEngine {
        assert_eq!(
            clock.high_water(),
            Stamp::NONE,
            "the engine starts on a clock that minted nothing: its views miss every earlier commit"
        );
        let (requests, inbox) = mpsc::channel();
        thread::Builder::new()
            .name("holon-views".into())
            .spawn(move || {
                let catalog = catalog();
                let plans =
                    check_all(&views().plans(), &catalog).expect("Holon's views are well typed");
                let on_stop = Box::new(on_stop);
                match RowKind::for_plans(&plans) {
                    RowKind::Dyn => {
                        Host::<DynRow>::new(clock, on_stop, &catalog, &plans).serve(inbox)
                    }
                }
            })
            .expect("the engine thread starts");
        ViewEngine { requests }
    }

    /// The view at the last released version, as the batch from the empty
    /// view, and the batches of every version released after it.
    pub fn snapshot_and_subscribe(
        &self,
        view: View,
    ) -> Result<(ViewBatch, Receiver<ViewBatch>), EngineError> {
        let (subscriber, batches) = mpsc::channel();
        let (reply, answer) = mpsc::channel();
        self.post(Request::Snapshot {
            view,
            subscriber,
            reply,
        });
        let state = answer
            .recv()
            .expect("the engine thread answers every snapshot")?;
        Ok((state, batches))
    }

    /// `delta` holds every block of `source` whose row changed in its commits
    /// stamped up to `cover`, with `None` for a deleted block.
    pub fn feed(
        &self,
        source: CommitSource,
        cover: Stamp,
        delta: Vec<(EntityUri, Option<SnapshotBlock>)>,
    ) {
        assert_ne!(source, CommitSource::Sql, "Sql feeds only focus_roots");
        self.post_feed(source, cover, Rows::Delta(delta));
    }

    /// `blocks` is every block of `source` after its commits stamped up to
    /// `cover`.
    pub fn replace(&self, source: CommitSource, cover: Stamp, blocks: Vec<SnapshotBlock>) {
        assert_ne!(source, CommitSource::Sql, "Sql feeds only focus_roots");
        self.post_feed(source, cover, Rows::Replace(blocks));
    }

    /// `delta` holds every focus root, by its history id, whose row changed in
    /// the Sql commits stamped up to `cover`, with `None` for a closed one.
    pub fn feed_focus_roots(&self, cover: Stamp, delta: Vec<(i64, Option<FocusRoot>)>) {
        self.post_feed(CommitSource::Sql, cover, Rows::FocusDelta(delta));
    }

    /// `roots` is every focus root after the Sql commits stamped up to `cover`.
    pub fn replace_focus_roots(&self, cover: Stamp, roots: Vec<FocusRoot>) {
        self.post_feed(CommitSource::Sql, cover, Rows::FocusReplace(roots));
    }

    /// Stops the engine with `error`; `source`'s commits stamped up to `cover`
    /// count as fed.
    pub fn stop(&self, source: CommitSource, cover: Stamp, error: EngineError) {
        self.post_feed(source, cover, Rows::Refused(error));
    }

    fn post_feed(&self, source: CommitSource, cover: Stamp, rows: Rows) {
        self.post(Request::Feed {
            source,
            cover,
            rows,
        });
    }

    fn post(&self, request: Request) {
        self.requests
            .send(request)
            .expect("the engine thread serves until the engine is dropped");
    }
}

/// Raises [`ConditionKind::ViewEngineStopped`] on `conditions`; an `on_stop`
/// for [`ViewEngine::start`].
pub fn raise_on(conditions: Arc<ConditionBus>) -> impl FnOnce(&EngineError) + Send + 'static {
    move |error| {
        conditions.emit(Condition {
            subject: VIEW_ENGINE_SUBJECT.to_string(),
            reason: ConditionKind::ViewEngineStopped(error.to_string()),
        })
    }
}

/// `row` of a relation or view with `schema`, its ids resolved by `interner`.
pub fn fields<R: Row>(row: &R, schema: &Schema, interner: &Interner) -> Vec<Field> {
    let layout = R::layout(schema);
    (0..schema.arity())
        .map(|i| match row.get(&layout, col(i)) {
            Datum::Bool(b) => Field::Bool(b),
            Datum::Int(n) => Field::Int(n),
            Datum::Id(id) => Field::Uri(interner.uri(id).clone()),
            Datum::Text(t) => Field::Text(t),
            Datum::Payload(p) => Field::Payload(p),
        })
        .collect()
}

/// A fed row: a block by the source that holds it, or a focus root by its
/// history id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Block(CommitSource, Id),
    Focus(i64),
}

impl Key {
    fn source(&self) -> CommitSource {
        match self {
            Key::Block(source, _) => *source,
            Key::Focus(_) => CommitSource::Sql,
        }
    }
}

/// One feed: the updates of one source's commits to its relation.
struct Batch<R> {
    source: CommitSource,
    /// The first and last stamp of the commits it holds; `None` for none.
    stamps: Option<(Stamp, Stamp)>,
    updates: Vec<(R, isize)>,
}

struct Host<R: Row> {
    clock: Arc<CommitClock>,
    on_stop: Option<Box<dyn FnOnce(&EngineError) + Send>>,
    worker: Worker,
    dataflow: Dataflow<R>,
    /// In the order of [`View::ALL`].
    schemas: Vec<Schema>,
    blocks: R::Layout,
    focus_roots: R::Layout,
    interner: Interner,
    /// Every fed row. A block that changes source is held by both until the
    /// leaving source feeds its retraction.
    current: HashMap<Key, R>,
    /// The parents in the last released version.
    parents: HashMap<Id, Id>,
    /// Fed batches not yet in a released version, in feed order.
    buffered: Vec<Batch<R>>,
    /// The `below` of the last released version.
    released: Stamp,
    /// Each view at `released`, in the order of [`View::ALL`].
    states: Vec<Multiset<R>>,
    subscribers: Vec<(View, Sender<ViewBatch>)>,
    stopped: Option<EngineError>,
}

impl<R: Row> Host<R> {
    fn new(
        clock: Arc<CommitClock>,
        on_stop: Box<dyn FnOnce(&EngineError) + Send>,
        catalog: &Catalog,
        plans: &[Rc<Checked>],
    ) -> Self {
        let mut worker = Worker::new(
            WorkerConfig::default(),
            Allocator::Thread(Thread::default()),
            None,
        );
        let dataflow = Dataflow::build(&mut worker, catalog, plans);
        Host {
            released: clock.low_watermark(),
            clock,
            on_stop: Some(on_stop),
            states: vec![Multiset::new(); plans.len()],
            worker,
            dataflow,
            schemas: plans.iter().map(|p| p.schema().clone()).collect(),
            blocks: R::layout(&blocks_schema()),
            focus_roots: R::layout(&focus_roots_schema()),
            interner: Interner::default(),
            current: HashMap::new(),
            parents: HashMap::new(),
            buffered: Vec::new(),
            subscribers: Vec::new(),
            stopped: None,
        }
    }

    fn serve(mut self, inbox: Receiver<Request>) {
        for request in inbox {
            match request {
                Request::Snapshot {
                    view,
                    subscriber,
                    reply,
                } => {
                    let state = self.unless_stopped(|host| Ok(host.snapshot(view, subscriber)));
                    reply
                        .send(state)
                        .expect("the caller waits for the snapshot");
                }
                Request::Feed {
                    source,
                    cover,
                    rows,
                } => {
                    // The engine may have failed or panicked before its own
                    // feed of the clock; a second feed through one cover is a
                    // no-op.
                    let fed = self.unless_stopped(|host| host.feed(source, cover, rows));
                    if fed.is_err() {
                        if let Err(refused) = self.clock.feed_through(source, cover) {
                            tracing::error!(
                                "the stopped view engine could not feed the clock: {refused}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// The stop error once stopped; else `handle`'s result, and its error or
    /// panic stops the engine.
    fn unless_stopped<T>(
        &mut self,
        handle: impl FnOnce(&mut Self) -> Result<T, EngineError>,
    ) -> Result<T, EngineError> {
        if let Some(error) = &self.stopped {
            return Err(error.clone());
        }
        // ALLOW(catch_unwind): a panic stops the engine; then only the clock is used.
        let result = panic::catch_unwind(AssertUnwindSafe(|| handle(self)))
            .unwrap_or_else(|panic| Err(EngineError::Panicked(panic_message(&*panic))));
        if let Err(error) = &result {
            (self.on_stop.take().expect("the engine stops once"))(error);
            self.stopped = Some(error.clone());
        }
        result
    }

    fn snapshot(&mut self, view: View, subscriber: Sender<ViewBatch>) -> ViewBatch {
        let i = View::ALL
            .iter()
            .position(|v| *v == view)
            .expect("View::ALL holds every view");
        let deltas = self.states[i]
            .iter()
            .map(|(row, n)| (fields(row, &self.schemas[i], &self.interner), *n))
            .collect();
        self.subscribers.push((view, subscriber));
        ViewBatch {
            below: self.released,
            deltas,
        }
    }

    /// The batch is buffered before the clock learns it is fed, so the low
    /// watermark never passes a commit the engine does not hold.
    fn feed(&mut self, source: CommitSource, cover: Stamp, rows: Rows) -> Result<(), EngineError> {
        let stamps: Vec<Stamp> = self
            .clock
            .outstanding(source)
            .into_iter()
            .take_while(|s| *s <= cover)
            .collect();
        let mut updates = Vec::new();
        let mut fed = HashSet::new();
        let replace = matches!(rows, Rows::Replace(_) | Rows::FocusReplace(_));
        match rows {
            Rows::Delta(delta) => {
                for (uri, block) in delta {
                    let key = Key::Block(source, self.interner.intern(&uri));
                    let row = block
                        .map(|b| block_row(&mut self.interner, &b))
                        .transpose()?;
                    self.set(key, row, &mut updates);
                }
            }
            Rows::Replace(blocks) => {
                for block in blocks {
                    let key = Key::Block(source, self.interner.intern(&block.block.id));
                    let row = block_row(&mut self.interner, &block)?;
                    self.set(key.clone(), Some(row), &mut updates);
                    fed.insert(key);
                }
            }
            Rows::FocusDelta(delta) => {
                for (history, focus) in delta {
                    let row = focus.map(|f| focus_root_row(&mut self.interner, &f));
                    self.set(Key::Focus(history), row, &mut updates);
                }
            }
            Rows::FocusReplace(roots) => {
                for focus in roots {
                    let key = Key::Focus(focus.history);
                    let row = focus_root_row(&mut self.interner, &focus);
                    self.set(key.clone(), Some(row), &mut updates);
                    fed.insert(key);
                }
            }
            Rows::Refused(error) => return Err(error),
        }
        if replace {
            let gone: Vec<Key> = self
                .current
                .keys()
                .filter(|key| key.source() == source && !fed.contains(*key))
                .cloned()
                .collect();
            for key in gone {
                self.set(key, None, &mut updates);
            }
        }
        if stamps.is_empty() {
            self.refuse_stampless_change(source, &updates)?;
        }
        self.buffered.push(Batch {
            source,
            stamps: stamps.first().copied().zip(stamps.last().copied()),
            updates,
        });
        self.clock.feed_through(source, cover)?;
        self.release()
    }

    fn set(&mut self, key: Key, row: Option<R>, updates: &mut Vec<(R, isize)>) {
        let old = match &row {
            Some(row) => self.current.insert(key, row.clone()),
            None => self.current.remove(&key),
        };
        if let Some(old) = old {
            updates.push((old, -1));
        }
        if let Some(row) = row {
            updates.push((row, 1));
        }
    }

    /// A batch with no stamps is released at any cut, so its updates must
    /// cancel: it may only repeat rows its source fed.
    fn refuse_stampless_change(
        &self,
        store: CommitSource,
        updates: &[(R, isize)],
    ) -> Result<(), EngineError> {
        const SAMPLE: usize = 5;
        let mut net = Multiset::new();
        for (row, diff) in updates {
            add(&mut net, row.clone(), *diff);
        }
        let changed: BTreeSet<String> = net.keys().map(|row| self.key_of(store, row)).collect();
        if changed.is_empty() {
            return Ok(());
        }
        Err(EngineError::StamplessChange {
            store,
            changed: changed.len(),
            sample: changed.into_iter().take(SAMPLE).collect(),
        })
    }

    /// Releases the newest version below the low watermark that splits no
    /// buffered batch: a batch holds a source's rows after its last commit,
    /// so a version between its first and last commit cannot be built.
    fn release(&mut self) -> Result<(), EngineError> {
        let mut below = self.clock.low_watermark();
        while let Some((first, _)) = self
            .buffered
            .iter()
            .filter_map(|b| b.stamps)
            .find(|(first, last)| *first < below && *last >= below)
        {
            below = first;
        }
        if below <= self.released {
            return Ok(());
        }
        // A source's batches go in in feed order. A batch with no stamps
        // cancels at any cut.
        let mut held = HashSet::new();
        let (now, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.buffered)
            .into_iter()
            .partition(|b| {
                let now =
                    !held.contains(&b.source) && b.stamps.is_none_or(|(_, last)| last < below);
                if !now {
                    held.insert(b.source);
                }
                now
            });
        self.buffered = later;
        self.dataflow.advance_to(below.get() - 1);
        let (mut updates, mut focus_roots) = (Multiset::new(), Multiset::new());
        for batch in now {
            let into = match batch.source {
                CommitSource::Sql => &mut focus_roots,
                CommitSource::LoroGlobal | CommitSource::LoroLayout => &mut updates,
            };
            for (row, diff) in batch.updates {
                add(into, row, diff);
            }
        }
        // A block that changes source comes in as a retraction from one
        // source and an insertion from the other, in either feed order.
        let (retracted, inserted): (Vec<_>, Vec<_>) =
            updates.into_iter().partition(|(_, diff)| *diff < 0);
        let mut moved = Vec::new();
        for (row, diff) in retracted.into_iter().chain(inserted) {
            let (id, parent) = (self.id(&row, ID), self.id(&row, PARENT));
            if diff > 0 {
                let held = self.parents.insert(id, parent);
                assert!(
                    held.is_none(),
                    "block {} is live in two sources at the version below {below:?}",
                    self.interner.uri(id)
                );
                moved.push(id);
            } else {
                assert_eq!(self.parents.remove(&id), Some(parent));
            }
            self.dataflow.update(BLOCKS, row, diff);
        }
        self.refuse_cycles(moved)?;
        for (row, diff) in focus_roots {
            self.dataflow.update(FOCUS_ROOTS, row, diff);
        }
        self.dataflow.commit(&mut self.worker)?;
        self.released = below;
        for (i, view) in View::ALL.into_iter().enumerate() {
            let mut deltas = Vec::new();
            for (row, diff) in self.dataflow.take_changes(i)? {
                deltas.push((fields(&row, &self.schemas[i], &self.interner), diff));
                add(&mut self.states[i], row, diff);
            }
            let batch = ViewBatch { below, deltas };
            // A dropped receiver unsubscribes.
            self.subscribers
                .retain(|(v, tx)| *v != view || tx.send(batch.clone()).is_ok());
        }
        Ok(())
    }

    /// The key of a row `store` fed, as text.
    fn key_of(&self, store: CommitSource, row: &R) -> String {
        match store {
            CommitSource::Sql => match row.get(&self.focus_roots, HISTORY) {
                Datum::Int(history) => history.to_string(),
                other => unreachable!("a focus_roots history holds {other:?}"),
            },
            CommitSource::LoroGlobal | CommitSource::LoroLayout => {
                self.interner.uri(self.id(row, ID)).to_string()
            }
        }
    }

    fn id(&self, row: &R, col: Col) -> Id {
        match row.get(&self.blocks, col) {
            Datum::Id(id) => id,
            other => unreachable!("a blocks id column holds {other:?}"),
        }
    }

    /// Every cycle in the released parents passes a block in `moved`, since
    /// the version before had none.
    fn refuse_cycles(&self, moved: Vec<Id>) -> Result<(), EngineError> {
        let mut acyclic = HashSet::new();
        for start in moved {
            let (mut path, mut on_path) = (Vec::new(), HashSet::new());
            let mut at = start;
            while !acyclic.contains(&at) {
                if !on_path.insert(at) {
                    let from = path.iter().position(|p| *p == at).expect("on the path");
                    let ids = path[from..]
                        .iter()
                        .map(|id| self.interner.uri(*id).clone())
                        .collect();
                    return Err(EngineError::ParentCycle { ids });
                }
                path.push(at);
                match self.parents.get(&at) {
                    Some(parent) => at = *parent,
                    None => break,
                }
            }
            acyclic.extend(path);
        }
        Ok(())
    }
}

fn panic_message(panic: &(dyn Any + Send)) -> String {
    match (panic.downcast_ref::<&str>(), panic.downcast_ref::<String>()) {
        (Some(message), _) => message.to_string(),
        (None, Some(message)) => message.clone(),
        (None, None) => "a panic payload that is not a string".to_string(),
    }
}
