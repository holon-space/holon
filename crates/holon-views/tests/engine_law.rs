//! The engine host over two sources on one commit clock: every version it
//! releases equals the batch backend over the authority state at that
//! version, and no version is released while a commit at or below it is
//! outstanding.
#![cfg(not(target_family = "wasm"))]

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::thread;
use std::time::Duration;

use holon_api::Block;
use holon_api::EntityUri;
use holon_api::block::SnapshotBlock;
use holon_api::commit_clock::CommitClock;
use holon_api::commit_clock::CommitSource;
use holon_views::batch;
use holon_views::batch::Multiset;
use holon_views::batch::add;
use holon_views::engine::Field;
use holon_views::engine::View;
use holon_views::engine::ViewBatch;
use holon_views::engine::ViewEngine;
use holon_views::engine::fields;
use holon_views::error::EngineError;
use holon_views::error::MAX_DEPTH;
use holon_views::intern::Interner;
use holon_views::plan::Checked;
use holon_views::plan::check_all;
use holon_views::row::DynRow;
use holon_views::views::block_row;
use holon_views::views::catalog;
use holon_views::views::views;
use proptest::collection::vec;
use proptest::option;
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

const SOURCES: [CommitSource; 2] = [CommitSource::LoroGlobal, CommitSource::LoroLayout];
const DEADLINE: Duration = Duration::from_secs(10);

fn uri(n: u32) -> EntityUri {
    EntityUri::block(&format!("b{n}"))
}

fn snapshot(n: u32, parent: Option<u32>, page: bool) -> SnapshotBlock {
    let mut block = Block {
        id: uri(n),
        parent_id: parent.map_or_else(EntityUri::no_parent, uri),
        created_at: 0,
        updated_at: 0,
        ..Block::default()
    };
    block.set_page(page);
    SnapshotBlock {
        block,
        sort_key: format!("{n:02x}"),
    }
}

#[derive(Debug, Clone)]
struct Node {
    source: usize,
    /// `None` under the root sentinel.
    parent: Option<u32>,
    page: bool,
}

/// The authority: blocks of both sources in one forest. A parent may sit in
/// the other source.
#[derive(Debug, Clone, Default)]
struct World {
    nodes: BTreeMap<u32, Node>,
    next: u32,
}

/// One edit by one source. A `usize` picks a block by position among the
/// candidates; with no candidate the edit changes nothing.
#[derive(Debug, Clone)]
enum Edit {
    Create {
        parent: Option<usize>,
        page: bool,
    },
    /// To the root, or to a block outside the moved subtree.
    Move {
        node: usize,
        parent: Option<usize>,
    },
    TogglePage(usize),
    DeleteLeaf(usize),
}

#[derive(Debug, Clone)]
enum Event {
    /// One commit, stamped by the clock.
    Commit(usize, Edit),
    /// The source's rows changed since its last feed, covering every stamp
    /// minted so far.
    Feed(usize),
    /// Every row of the source.
    Replace(usize),
}

fn pick(candidates: Vec<u32>, i: usize) -> Option<u32> {
    (!candidates.is_empty()).then(|| candidates[i % candidates.len()])
}

impl World {
    fn of(&self, source: usize) -> Vec<u32> {
        self.nodes
            .iter()
            .filter(|(_, node)| node.source == source)
            .map(|(n, _)| *n)
            .collect()
    }

    fn subtree(&self, root: u32) -> BTreeSet<u32> {
        let mut out = BTreeSet::from([root]);
        loop {
            let more: Vec<u32> = self
                .nodes
                .iter()
                .filter(|(n, node)| {
                    !out.contains(n) && node.parent.is_some_and(|p| out.contains(&p))
                })
                .map(|(n, _)| *n)
                .collect();
            if more.is_empty() {
                return out;
            }
            out.extend(more);
        }
    }

    /// The block the edit changed, if any.
    fn apply(&mut self, source: usize, edit: &Edit) -> Option<u32> {
        let all: Vec<u32> = self.nodes.keys().copied().collect();
        match edit {
            Edit::Create { parent, page } => {
                let parent = parent.and_then(|i| pick(all, i));
                let n = self.next;
                self.next += 1;
                self.nodes.insert(
                    n,
                    Node {
                        source,
                        parent,
                        page: *page,
                    },
                );
                Some(n)
            }
            Edit::Move { node, parent } => {
                let node = pick(self.of(source), *node)?;
                let subtree = self.subtree(node);
                let outside = all.into_iter().filter(|n| !subtree.contains(n)).collect();
                let parent = parent.and_then(|i| pick(outside, i));
                self.nodes.get_mut(&node).unwrap().parent = parent;
                Some(node)
            }
            Edit::TogglePage(i) => {
                let node = pick(self.of(source), *i)?;
                let node_ref = self.nodes.get_mut(&node).unwrap();
                node_ref.page = !node_ref.page;
                Some(node)
            }
            Edit::DeleteLeaf(i) => {
                let leaves = self
                    .of(source)
                    .into_iter()
                    .filter(|n| self.nodes.values().all(|c| c.parent != Some(*n)))
                    .collect();
                let node = pick(leaves, *i)?;
                self.nodes.remove(&node);
                Some(node)
            }
        }
    }

    fn snapshot(&self, n: u32) -> Option<SnapshotBlock> {
        self.nodes
            .get(&n)
            .map(|node| snapshot(n, node.parent, node.page))
    }
}

type Released = Multiset<Vec<Field>>;

/// The views over `world` by the batch backend, decoded.
fn expected(plans: &[Rc<Checked>], world: &World) -> Vec<Released> {
    let mut interner = Interner::default();
    let blocks: Multiset<DynRow> = world
        .nodes
        .keys()
        .map(|n| {
            let row = block_row(&mut interner, &world.snapshot(*n).unwrap()).unwrap();
            (row, 1)
        })
        .collect();
    plans
        .iter()
        .map(|plan| {
            batch::run(plan, std::slice::from_ref(&blocks))
                .expect("a generated forest is shallow")
                .into_iter()
                .map(|(row, n)| (fields(&row, plan.schema(), &interner), n))
                .collect()
        })
        .collect()
}

struct Harness {
    clock: Arc<CommitClock>,
    engine: ViewEngine,
    subscribers: Vec<Receiver<ViewBatch>>,
    plans: Vec<Rc<Checked>>,
    world: World,
    /// `states[k]`: the world after the commit stamped `k`.
    states: Vec<World>,
    pending: [BTreeSet<u32>; 2],
    views: Vec<Released>,
    /// The `below` of the last released version; 1 before the first.
    released: u64,
}

impl Harness {
    fn new() -> Harness {
        let clock = Arc::new(CommitClock::new());
        let engine = ViewEngine::start(clock.clone());
        let subscribers = View::ALL
            .iter()
            .map(|view| engine.subscribe(*view).unwrap())
            .collect();
        Harness {
            clock,
            engine,
            subscribers,
            plans: check_all(&views().plans(), &catalog()).unwrap(),
            world: World::default(),
            states: vec![World::default()],
            pending: Default::default(),
            views: vec![Released::new(); View::ALL.len()],
            released: 1,
        }
    }

    fn commit(&mut self, source: usize, edit: &Edit) {
        if let Some(n) = self.world.apply(source, edit) {
            self.clock.mint(SOURCES[source]);
            self.states.push(self.world.clone());
            self.pending[source].insert(n);
        }
    }

    fn feed(&mut self, source: usize) -> Result<(), EngineError> {
        let cover = self.clock.high_water();
        let delta = std::mem::take(&mut self.pending[source])
            .into_iter()
            .map(|n| (uri(n), self.world.snapshot(n)))
            .collect();
        self.engine.feed(SOURCES[source], cover, delta)
    }

    fn replace(&mut self, source: usize) -> Result<(), EngineError> {
        let cover = self.clock.high_water();
        self.pending[source].clear();
        let blocks = self
            .world
            .of(source)
            .into_iter()
            .map(|n| self.world.snapshot(n).unwrap())
            .collect();
        self.engine.replace(SOURCES[source], cover, blocks)
    }

    /// Takes every released version and checks it against the authority at
    /// that version.
    fn check_released(&mut self) -> Result<(), TestCaseError> {
        loop {
            let batches: Vec<Option<ViewBatch>> = self
                .subscribers
                .iter()
                .map(|rx| rx.try_recv().ok())
                .collect();
            if batches.iter().all(Option::is_none) {
                return Ok(());
            }
            let batches: Vec<ViewBatch> = batches
                .into_iter()
                .map(|b| b.expect("every view releases every version"))
                .collect();
            let below = batches[0].below;
            prop_assert!(batches.iter().all(|b| b.below == below));
            prop_assert!(
                below.get() > self.released,
                "{:?} after {}",
                below,
                self.released
            );
            prop_assert!(
                self.clock.low_watermark() >= below,
                "released below {:?} while {:?} is outstanding",
                below,
                self.clock.low_watermark()
            );
            self.released = below.get();
            for (view, batch) in self.views.iter_mut().zip(batches) {
                for (row, diff) in batch.deltas {
                    add(view, row, diff);
                }
            }
            let truth = &self.states[usize::try_from(below.get() - 1).unwrap()];
            prop_assert_eq!(
                &self.views,
                &expected(&self.plans, truth),
                "below {:?}",
                below
            );
        }
    }
}

fn law(events: Vec<Event>) -> Result<(), TestCaseError> {
    let mut h = Harness::new();
    for event in events {
        match event {
            Event::Commit(source, edit) => h.commit(source, &edit),
            Event::Feed(source) => h.feed(source).unwrap(),
            Event::Replace(source) => h.replace(source).unwrap(),
        }
        h.check_released()?;
    }
    for source in 0..SOURCES.len() {
        h.feed(source).unwrap();
    }
    h.check_released()?;
    let high = h.clock.high_water().get();
    if high > 0 {
        prop_assert_eq!(h.released, high + 1, "the last version is released");
    }
    Ok(())
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        3 => (option::of(any::<usize>()), any::<bool>())
            .prop_map(|(parent, page)| Edit::Create { parent, page }),
        2 => (any::<usize>(), option::of(any::<usize>()))
            .prop_map(|(node, parent)| Edit::Move { node, parent }),
        1 => any::<usize>().prop_map(Edit::TogglePage),
        1 => any::<usize>().prop_map(Edit::DeleteLeaf),
    ]
}

fn event() -> impl Strategy<Value = Event> {
    let source = 0..SOURCES.len();
    prop_oneof![
        5 => (source.clone(), edit()).prop_map(|(s, e)| Event::Commit(s, e)),
        2 => source.clone().prop_map(Event::Feed),
        1 => source.prop_map(Event::Replace),
    ]
}

fn config() -> ProptestConfig {
    ProptestConfig {
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/engine_law.proptest-regressions"
        )))),
        ..ProptestConfig::with_cases(512)
    }
}

proptest! {
    #![proptest_config(config())]
    #[test]
    fn released_versions_equal_the_authority(events in vec(event(), 1..40)) {
        law(events)?;
    }
}

fn within_deadline<T: Send + 'static>(case: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || tx.send(case()).unwrap());
    rx.recv_timeout(DEADLINE)
        .unwrap_or_else(|e| panic!("the case did not finish within {DEADLINE:?}: {e}"))
}

/// A page with a chain of `depth` non-pages below it, in one commit.
fn chain(depth: u32) -> Harness {
    let mut h = Harness::new();
    h.commit(
        0,
        &Edit::Create {
            parent: None,
            page: true,
        },
    );
    for i in 0..depth {
        h.commit(
            0,
            &Edit::Create {
                parent: Some(i as usize),
                page: false,
            },
        );
    }
    h
}

#[test]
fn a_200_deep_chain_resolves() {
    within_deadline(|| {
        let mut h = chain(200);
        h.feed(0).unwrap();
        h.check_released().unwrap();
        let owners = &h.views[View::OwningPage as usize];
        assert_eq!(owners.len(), 201);
        assert!(
            owners.keys().all(|row| row[1] == Field::Uri(uri(0))),
            "{owners:?}"
        );
    });
}

#[test]
fn a_chain_past_max_depth_stops_the_engine() {
    within_deadline(|| {
        let mut h = chain(u32::try_from(MAX_DEPTH).unwrap() + 1);
        assert_eq!(h.feed(0), Err(EngineError::DepthBound));
        assert_eq!(h.feed(1), Err(EngineError::DepthBound));
    });
}

#[test]
fn a_parent_cycle_stops_the_engine() {
    within_deadline(|| {
        let mut h = Harness::new();
        h.commit(
            0,
            &Edit::Create {
                parent: None,
                page: false,
            },
        );
        h.commit(
            0,
            &Edit::Create {
                parent: Some(0),
                page: false,
            },
        );
        h.feed(0).unwrap();
        h.world.nodes.get_mut(&0).unwrap().parent = Some(1);
        h.clock.mint(SOURCES[0]);
        h.pending[0].insert(0);
        let Err(EngineError::ParentCycle { ids }) = h.feed(0) else {
            panic!("a cycle through b0 and b1 is refused");
        };
        assert_eq!(
            ids.into_iter().collect::<BTreeSet<_>>(),
            BTreeSet::from([uri(0), uri(1)])
        );
        assert!(matches!(
            h.engine.subscribe(View::Row),
            Err(EngineError::ParentCycle { .. })
        ));
    });
}

/// Fed but unreleased rows of two sources can form a cycle that no version
/// of the authority holds.
#[test]
fn a_cycle_only_between_unreleased_feeds_is_not_refused() {
    within_deadline(|| {
        let mut h = Harness::new();
        h.commit(
            0,
            &Edit::Create {
                parent: None,
                page: false,
            },
        );
        h.commit(
            1,
            &Edit::Create {
                parent: None,
                page: false,
            },
        );
        h.feed(0).unwrap();
        h.feed(1).unwrap();
        h.commit(
            1,
            &Edit::Create {
                parent: None,
                page: false,
            },
        );
        h.commit(
            0,
            &Edit::Move {
                node: 0,
                parent: Some(0),
            },
        );
        h.feed(0).unwrap();
        h.commit(
            0,
            &Edit::Move {
                node: 0,
                parent: None,
            },
        );
        h.commit(
            1,
            &Edit::Move {
                node: 0,
                parent: Some(0),
            },
        );
        // Fed now: b0 under b1 (stamp 4) and b1 under b0 (stamp 6).
        h.feed(1).unwrap();
        h.feed(0).unwrap();
        h.check_released().unwrap();
        assert_eq!(h.released, h.clock.high_water().get() + 1);
        assert_eq!(h.world.nodes[&1].parent, Some(0));
    });
}

#[test]
fn a_panic_on_the_engine_thread_stops_every_call() {
    within_deadline(|| {
        let mut h = Harness::new();
        h.commit(
            0,
            &Edit::Create {
                parent: None,
                page: false,
            },
        );
        h.feed(0).unwrap();
        h.clock.mint(SOURCES[1]);
        let cover = h.clock.high_water();
        let stolen = vec![(uri(0), h.world.snapshot(0))];
        assert_eq!(
            h.engine.feed(SOURCES[1], cover, stolen),
            Err(EngineError::Stopped)
        );
        assert_eq!(h.feed(0), Err(EngineError::Stopped));
        assert_eq!(
            h.engine.subscribe(View::Row).err(),
            Some(EngineError::Stopped)
        );
    });
}
