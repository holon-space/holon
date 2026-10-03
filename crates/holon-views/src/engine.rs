//! The engine host: the views of one dataflow on a thread of their own, fed
//! per source and released per version of the commit clock.

use std::collections::HashMap;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;
use std::thread;

use holon_api::EntityUri;
use holon_api::block::SnapshotBlock;
use holon_api::commit_clock::CommitClock;
use holon_api::commit_clock::CommitSource;
use holon_api::commit_clock::Stamp;
use timely::WorkerConfig;
use timely::communication::Allocator;
use timely::communication::allocator::Thread;
use timely::worker::Worker;

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
use crate::views::ID;
use crate::views::PARENT;
use crate::views::block_row;
use crate::views::blocks_schema;
use crate::views::catalog;
use crate::views::views;

/// In the order of [`crate::views::Views::plans`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Children,
    OwningPage,
    Row,
}

impl View {
    pub const ALL: [View; 3] = [View::Children, View::OwningPage, View::Row];
}

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

/// After the first error every call returns it; after a panic on the engine
/// thread every call returns [`EngineError::Stopped`].
pub struct ViewEngine {
    requests: Sender<(Request, Sender<Result<(), EngineError>>)>,
}

enum Request {
    Subscribe(View, Sender<ViewBatch>),
    Feed {
        source: CommitSource,
        cover: Stamp,
        rows: Rows,
    },
}

enum Rows {
    Delta(Vec<(EntityUri, Option<SnapshotBlock>)>),
    /// Every row of the source; a fed row not among them is deleted.
    Replace(Vec<SnapshotBlock>),
}

impl ViewEngine {
    pub fn start(clock: Arc<CommitClock>) -> ViewEngine {
        let (requests, inbox) = mpsc::channel();
        thread::Builder::new()
            .name("holon-views".into())
            .spawn(move || {
                let catalog = catalog();
                let plans =
                    check_all(&views().plans(), &catalog).expect("Holon's views are well typed");
                match RowKind::for_plans(&plans) {
                    RowKind::Dyn => Host::<DynRow>::new(clock, &catalog, &plans).serve(inbox),
                }
            })
            .expect("the engine thread starts");
        ViewEngine { requests }
    }

    /// The batches of every version released after this call.
    pub fn subscribe(&self, view: View) -> Result<Receiver<ViewBatch>, EngineError> {
        let (tx, rx) = mpsc::channel();
        self.call(Request::Subscribe(view, tx))?;
        Ok(rx)
    }

    /// `delta` holds every block of `source` whose row changed in its commits
    /// stamped up to `cover`, with `None` for a deleted block.
    pub fn feed(
        &self,
        source: CommitSource,
        cover: Stamp,
        delta: Vec<(EntityUri, Option<SnapshotBlock>)>,
    ) -> Result<(), EngineError> {
        let rows = Rows::Delta(delta);
        self.call(Request::Feed {
            source,
            cover,
            rows,
        })
    }

    /// `blocks` is every block of `source` after its commits stamped up to
    /// `cover`.
    pub fn replace(
        &self,
        source: CommitSource,
        cover: Stamp,
        blocks: Vec<SnapshotBlock>,
    ) -> Result<(), EngineError> {
        let rows = Rows::Replace(blocks);
        self.call(Request::Feed {
            source,
            cover,
            rows,
        })
    }

    fn call(&self, request: Request) -> Result<(), EngineError> {
        let (reply, answer) = mpsc::channel();
        self.requests
            .send((request, reply))
            .map_err(|_| EngineError::Stopped)?;
        answer.recv().map_err(|_| EngineError::Stopped)?
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

/// One feed: the `blocks` updates of one source's commits.
struct Batch<R> {
    source: CommitSource,
    /// The first and last stamp of the commits it holds; `None` for none.
    stamps: Option<(Stamp, Stamp)>,
    updates: Vec<(R, isize)>,
}

struct Host<R: Row> {
    clock: Arc<CommitClock>,
    worker: Worker,
    dataflow: Dataflow<R>,
    /// By [`View`].
    schemas: Vec<Schema>,
    blocks: R::Layout,
    interner: Interner,
    /// Every fed row, with the source that fed it.
    current: HashMap<Id, (CommitSource, R)>,
    /// The parents in the last released version.
    parents: HashMap<Id, Id>,
    /// Fed batches not yet in a released version, in feed order.
    buffered: Vec<Batch<R>>,
    /// The `below` of the last released version.
    released: Stamp,
    subscribers: Vec<(View, Sender<ViewBatch>)>,
    stopped: Option<EngineError>,
}

impl<R: Row> Host<R> {
    fn new(clock: Arc<CommitClock>, catalog: &Catalog, plans: &[Rc<Checked>]) -> Self {
        let mut worker = Worker::new(
            WorkerConfig::default(),
            Allocator::Thread(Thread::default()),
            None,
        );
        let dataflow = Dataflow::build(&mut worker, catalog, plans);
        Host {
            released: clock.low_watermark(),
            clock,
            worker,
            dataflow,
            schemas: plans.iter().map(|p| p.schema().clone()).collect(),
            blocks: R::layout(&blocks_schema()),
            interner: Interner::default(),
            current: HashMap::new(),
            parents: HashMap::new(),
            buffered: Vec::new(),
            subscribers: Vec::new(),
            stopped: None,
        }
    }

    fn serve(mut self, inbox: Receiver<(Request, Sender<Result<(), EngineError>>)>) {
        for (request, reply) in inbox {
            let result = match &self.stopped {
                Some(error) => Err(error.clone()),
                None => self.handle(request),
            };
            if let Err(error) = &result {
                self.stopped.get_or_insert_with(|| error.clone());
            }
            reply.send(result).expect("the caller waits for the reply");
        }
    }

    fn handle(&mut self, request: Request) -> Result<(), EngineError> {
        match request {
            Request::Subscribe(view, tx) => {
                self.subscribers.push((view, tx));
                Ok(())
            }
            Request::Feed {
                source,
                cover,
                rows,
            } => self.feed(source, cover, rows),
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
        match rows {
            Rows::Delta(delta) => {
                for (uri, block) in delta {
                    let id = self.interner.intern(&uri);
                    let row = block
                        .map(|b| block_row(&mut self.interner, &b))
                        .transpose()?;
                    self.set(source, id, row, &mut updates);
                }
            }
            Rows::Replace(blocks) => {
                let mut fed = HashSet::new();
                for block in blocks {
                    let id = self.interner.intern(&block.block.id);
                    fed.insert(id);
                    let row = block_row(&mut self.interner, &block)?;
                    self.set(source, id, Some(row), &mut updates);
                }
                let gone: Vec<Id> = self
                    .current
                    .iter()
                    .filter(|(id, (owner, _))| *owner == source && !fed.contains(id))
                    .map(|(id, _)| *id)
                    .collect();
                for id in gone {
                    self.set(source, id, None, &mut updates);
                }
            }
        }
        self.buffered.push(Batch {
            source,
            stamps: stamps.first().copied().zip(stamps.last().copied()),
            updates,
        });
        self.clock.feed_through(source, cover);
        self.release()
    }

    fn set(&mut self, source: CommitSource, id: Id, row: Option<R>, updates: &mut Vec<(R, isize)>) {
        let old = match &row {
            Some(row) => self.current.insert(id, (source, row.clone())),
            None => self.current.remove(&id),
        };
        if let Some((owner, old)) = old {
            assert_eq!(
                owner,
                source,
                "block {} is fed by two sources",
                self.interner.uri(id)
            );
            updates.push((old, -1));
        }
        if let Some(row) = row {
            updates.push((row, 1));
        }
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
        // A source's batches go in in feed order.
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
        let mut moved = Vec::new();
        for batch in now {
            for (row, diff) in batch.updates {
                let (id, parent) = (self.id(&row, ID), self.id(&row, PARENT));
                if diff > 0 {
                    self.parents.insert(id, parent);
                    moved.push(id);
                } else {
                    assert_eq!(self.parents.remove(&id), Some(parent));
                }
                self.dataflow.update(BLOCKS, row, diff);
            }
        }
        self.refuse_cycles(moved)?;
        self.dataflow.commit(&mut self.worker)?;
        self.released = below;
        for (i, view) in View::ALL.into_iter().enumerate() {
            let deltas: Vec<(Vec<Field>, isize)> = self
                .dataflow
                .take_changes(i)?
                .into_iter()
                .map(|(row, diff)| (fields(&row, &self.schemas[i], &self.interner), diff))
                .collect();
            let batch = ViewBatch { below, deltas };
            // A dropped receiver unsubscribes.
            self.subscribers
                .retain(|(v, tx)| *v != view || tx.send(batch.clone()).is_ok());
        }
        Ok(())
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
