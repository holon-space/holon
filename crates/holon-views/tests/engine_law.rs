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
use holon_api::ConditionBus;
use holon_api::ConditionKind;
use holon_api::EntityUri;
use holon_api::block::SnapshotBlock;
use holon_api::commit_clock::CommitClock;
use holon_api::commit_clock::CommitSource;
use holon_api::commit_clock::CoverAboveHighWater;
use holon_api::commit_clock::Stamp;
use holon_api::condition_bus::VIEW_ENGINE_SUBJECT;
use holon_views::batch;
use holon_views::batch::Multiset;
use holon_views::batch::add;
use holon_views::engine::Field;
use holon_views::engine::ViewBatch;
use holon_views::engine::ViewEngine;
use holon_views::engine::fields;
use holon_views::engine::raise_on;
use holon_views::error::EngineError;
use holon_views::error::MAX_DEPTH;
use holon_views::intern::Interner;
use holon_views::plan::Checked;
use holon_views::plan::check_all;
use holon_views::row::DynRow;
use holon_views::views::View;
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
    /// A leaf of the other source leaves it in one commit and joins this
    /// source in the next, under the same parent.
    Adopt(usize),
}

/// How far a feed covers: `Cut(p)` is `p` percent of the way from the
/// source's last cover to the high water. A cut that passes none of the
/// source's commits feeds no stamp.
#[derive(Debug, Clone, Copy)]
struct Cut(u8);

#[derive(Debug, Clone)]
enum Event {
    /// One commit, stamped by the clock.
    Commit(usize, Edit),
    /// The source's rows changed by its commits up to the cut.
    Feed(usize, Cut),
    /// Every row of the source after its commits up to the cut.
    Replace(usize, Cut),
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

    fn leaves(&self, source: usize) -> Vec<u32> {
        self.of(source)
            .into_iter()
            .filter(|n| self.nodes.values().all(|c| c.parent != Some(*n)))
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
                let node = pick(self.leaves(source), *i)?;
                self.nodes.remove(&node);
                Some(node)
            }
            Edit::Adopt(_) => unreachable!("an adoption is two commits"),
        }
    }

    fn snapshot(&self, n: u32) -> Option<SnapshotBlock> {
        self.nodes
            .get(&n)
            .map(|node| snapshot(n, node.parent, node.page))
    }

    fn snapshot_of(&self, source: usize, n: u32) -> Option<SnapshotBlock> {
        self.snapshot(n).filter(|_| self.nodes[&n].source == source)
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
    /// `states[k]`: the world after the commit stamped `stamps[k]`;
    /// `stamps[0]` is [`Stamp::NONE`].
    states: Vec<World>,
    stamps: Vec<Stamp>,
    /// Per source: the commits no feed covered yet, as `(k, block)`.
    pending: [Vec<(usize, u32)>; 2],
    /// Per source: the `k` of its last cover.
    covered: [usize; 2],
    views: Vec<Released>,
    /// The `below` of the last released version; 1 before the first.
    released: u64,
}

impl Harness {
    fn new() -> Harness {
        Harness::with_engine(|clock| ViewEngine::start(clock, |_| {}))
    }

    fn with_engine(start: impl FnOnce(Arc<CommitClock>) -> ViewEngine) -> Harness {
        let clock = Arc::new(CommitClock::new());
        let engine = start(clock.clone());
        let subscribers = View::ALL
            .iter()
            .map(|view| {
                let (state, rx) = engine.snapshot_and_subscribe(*view).unwrap();
                assert!(state.deltas.is_empty());
                rx
            })
            .collect();
        Harness {
            clock,
            engine,
            subscribers,
            plans: check_all(&views().plans(), &catalog()).unwrap(),
            world: World::default(),
            states: vec![World::default()],
            stamps: vec![Stamp::NONE],
            pending: Default::default(),
            covered: [0; 2],
            views: vec![Released::new(); View::ALL.len()],
            released: 1,
        }
    }

    fn commit(&mut self, source: usize, edit: &Edit) {
        if let Edit::Adopt(i) = edit {
            return self.adopt(source, *i);
        }
        if let Some(n) = self.world.apply(source, edit) {
            self.record(source, n);
        }
    }

    fn adopt(&mut self, source: usize, i: usize) {
        let other = 1 - source;
        let Some(n) = pick(self.world.leaves(other), i) else {
            return;
        };
        let node = self.world.nodes.remove(&n).unwrap();
        self.record(other, n);
        self.world.nodes.insert(n, Node { source, ..node });
        self.record(source, n);
    }

    /// Stamps the change of block `n` that `world` already holds.
    fn record(&mut self, source: usize, n: u32) {
        self.stamps.push(self.clock.mint(SOURCES[source]));
        self.states.push(self.world.clone());
        self.pending[source].push((self.states.len() - 1, n));
    }

    fn last(&self) -> usize {
        self.states.len() - 1
    }

    fn cut(&self, source: usize, Cut(percent): Cut) -> usize {
        let from = self.covered[source];
        from + (self.last() - from) * usize::from(percent) / 100
    }

    /// Waits until the engine handled every post before it.
    fn settle(&self) -> Result<(), EngineError> {
        self.engine.snapshot_and_subscribe(View::Row).map(drop)
    }

    fn feed(&mut self, source: usize) -> Result<(), EngineError> {
        self.feed_to(source, self.last())
    }

    fn feed_to(&mut self, source: usize, k: usize) -> Result<(), EngineError> {
        let (now, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending[source])
            .into_iter()
            .partition(|(at, _)| *at <= k);
        self.pending[source] = later;
        self.covered[source] = k;
        let delta = now
            .into_iter()
            .map(|(_, n)| n)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|n| (uri(n), self.states[k].snapshot_of(source, n)))
            .collect();
        self.engine.feed(SOURCES[source], self.stamps[k], delta);
        self.settle()
    }

    fn replace(&mut self, source: usize) -> Result<(), EngineError> {
        self.replace_to(source, self.last())
    }

    fn replace_to(&mut self, source: usize, k: usize) -> Result<(), EngineError> {
        self.pending[source].retain(|(at, _)| *at > k);
        self.covered[source] = k;
        let state = &self.states[k];
        let blocks = state
            .of(source)
            .into_iter()
            .map(|n| state.snapshot(n).unwrap())
            .collect();
        self.engine.replace(SOURCES[source], self.stamps[k], blocks);
        self.settle()
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
            Event::Feed(source, cut) => h.feed_to(source, h.cut(source, cut)).unwrap(),
            Event::Replace(source, cut) => h.replace_to(source, h.cut(source, cut)).unwrap(),
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
        1 => any::<usize>().prop_map(Edit::Adopt),
    ]
}

fn cut() -> impl Strategy<Value = Cut> {
    prop_oneof![1 => Just(100), 1 => 0u8..=100].prop_map(Cut)
}

fn event() -> impl Strategy<Value = Event> {
    let source = 0..SOURCES.len();
    prop_oneof![
        5 => (source.clone(), edit()).prop_map(|(s, e)| Event::Commit(s, e)),
        2 => (source.clone(), cut()).prop_map(|(s, c)| Event::Feed(s, c)),
        1 => (source, cut()).prop_map(|(s, c)| Event::Replace(s, c)),
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
        let owning = View::ALL
            .iter()
            .position(|v| *v == View::OwningPage)
            .unwrap();
        let owners = &h.views[owning];
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

/// b1 under b0, fed; then b0 under b1.
fn parent_cycle(h: &mut Harness) -> Result<(), EngineError> {
    create(h, 0, None);
    create(h, 0, Some(0));
    h.feed(0).unwrap();
    h.world.nodes.get_mut(&0).unwrap().parent = Some(1);
    h.record(0, 0);
    h.feed(0)
}

#[test]
fn a_parent_cycle_stops_the_engine() {
    within_deadline(|| {
        let mut h = Harness::new();
        let Err(EngineError::ParentCycle { ids }) = parent_cycle(&mut h) else {
            panic!("a cycle through b0 and b1 is refused");
        };
        assert_eq!(
            ids.into_iter().collect::<BTreeSet<_>>(),
            BTreeSet::from([uri(0), uri(1)])
        );
        assert!(matches!(
            h.engine.snapshot_and_subscribe(View::Row),
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
fn a_stampless_batch_that_changes_rows_stops_the_engine() {
    within_deadline(|| {
        let conditions = Arc::new(ConditionBus::new());
        let bus = conditions.clone();
        let mut h = Harness::with_engine(|clock| ViewEngine::start(clock, raise_on(bus)));
        create(&mut h, 0, None);
        h.feed(0).unwrap();
        h.replace(0).unwrap();
        h.engine.replace(
            SOURCES[0],
            h.stamps[1],
            vec![snapshot(0, None, true), snapshot(7, None, false)],
        );
        let stopped = EngineError::StamplessChange {
            store: SOURCES[0],
            changed: 2,
            sample: vec![uri(0), uri(7)],
        };
        assert_eq!(h.settle(), Err(stopped.clone()));
        assert_eq!(stops(&conditions), [stopped.to_string()]);
        every_stamp_is_fed(&h.clock);
    });
}

#[test]
fn a_feed_that_covers_no_commit_waits_behind_its_source() {
    within_deadline(|| {
        let mut h = Harness::new();
        let root = |page| Edit::Create { parent: None, page };
        h.commit(1, &root(false));
        h.commit(0, &root(true));
        h.commit(1, &Edit::TogglePage(0));
        h.commit(0, &Edit::TogglePage(0));
        h.feed(0).unwrap();
        h.replace(0).unwrap();
        h.feed_to(1, 1).unwrap();
        h.check_released().unwrap();
        assert_eq!(h.released, 2, "source 0's batch holds stamps 2 and 4");
        h.feed(1).unwrap();
        h.check_released().unwrap();
        assert_eq!(h.released, 5);
    });
}

/// The joining source is fed before the leaving one, so the engine holds
/// the block from both until the version that holds both commits.
#[test]
fn a_block_that_changes_source_is_retracted_and_inserted_in_one_version() {
    within_deadline(|| {
        let mut h = Harness::new();
        create(&mut h, 0, None);
        h.feed(0).unwrap();
        h.commit(1, &Edit::Adopt(0));
        h.feed(1).unwrap();
        h.feed(0).unwrap();
        h.check_released().unwrap();
        assert_eq!(h.released, h.clock.high_water().get() + 1);
        assert_eq!(h.world.nodes[&0].source, 1);
    });
}

#[test]
fn each_view_releases_the_rows_of_its_own_plan() {
    within_deadline(|| {
        let mut h = Harness::new();
        let subscribed: Vec<_> = View::ALL
            .iter()
            .map(|view| (*view, h.engine.snapshot_and_subscribe(*view).unwrap().1))
            .collect();
        h.commit(
            0,
            &Edit::Create {
                parent: None,
                page: true,
            },
        );
        h.feed(0).unwrap();
        for (view, rx) in subscribed {
            let batch = rx.try_recv().unwrap();
            let [(row, 1)] = batch.deltas.as_slice() else {
                panic!("{view:?} releases one row, got {:?}", batch.deltas);
            };
            let page = Field::Uri(uri(0));
            let fits = match view {
                View::Children => matches!(row.as_slice(), [_, Field::Text(_), id] if *id == page),
                View::OwningPage => row.as_slice() == [page.clone(), page],
                View::Row => matches!(row.as_slice(), [id, Field::Payload(_)] if *id == page),
            };
            assert!(fits, "{view:?} released {row:?}");
        }
    });
}

fn create(h: &mut Harness, source: usize, parent: Option<usize>) {
    h.commit(
        source,
        &Edit::Create {
            parent,
            page: false,
        },
    );
}

/// The stop reason the conditions name, one per raise.
fn stops(conditions: &ConditionBus) -> Vec<String> {
    conditions
        .current()
        .into_iter()
        .map(|c| match c.reason {
            ConditionKind::ViewEngineStopped(reason) if c.subject == VIEW_ENGINE_SUBJECT => reason,
            other => panic!("{} raised {other:?}", c.subject),
        })
        .collect()
}

fn every_stamp_is_fed(clock: &CommitClock) {
    assert_eq!(
        clock.low_watermark().get(),
        clock.high_water().get() + 1,
        "outstanding: {:?}",
        SOURCES.map(|s| clock.outstanding(s))
    );
}

#[test]
fn a_cover_past_the_high_water_stops_the_engine_but_not_the_clock() {
    within_deadline(|| {
        let mut h = Harness::new();
        let other = CommitClock::new();
        other.mint(SOURCES[0]);
        let cover = other.mint(SOURCES[0]);
        create(&mut h, 0, None);
        let refused = Err(EngineError::Clock(CoverAboveHighWater {
            store: SOURCES[0],
            cover,
            high_water: h.clock.high_water(),
        }));
        h.engine.feed(SOURCES[0], cover, vec![]);
        assert_eq!(h.settle(), refused);
        assert_eq!(h.clock.high_water().get(), 1);
        assert_eq!(h.clock.outstanding(SOURCES[0]).len(), 1);
        assert_eq!(h.feed(0), refused);
        every_stamp_is_fed(&h.clock);
    });
}

#[test]
fn a_stopped_engine_still_feeds_the_clock() {
    within_deadline(|| {
        let conditions = Arc::new(ConditionBus::new());
        let bus = conditions.clone();
        let mut h = Harness::with_engine(|clock| ViewEngine::start(clock, raise_on(bus)));
        assert!(matches!(
            parent_cycle(&mut h),
            Err(EngineError::ParentCycle { .. })
        ));
        create(&mut h, 0, None);
        create(&mut h, 1, None);
        create(&mut h, 1, None);
        h.feed(0).unwrap_err();
        h.replace_to(1, h.last() - 1).unwrap_err();
        h.feed(1).unwrap_err();
        every_stamp_is_fed(&h.clock);
        let [reason] = stops(&conditions).try_into().unwrap();
        assert!(reason.contains("form a cycle"), "{reason}");
    });
}

#[test]
fn a_panic_keeps_the_clock_fed() {
    within_deadline(|| {
        let conditions = Arc::new(ConditionBus::new());
        let bus = conditions.clone();
        let mut h = Harness::with_engine(|clock| ViewEngine::start(clock, raise_on(bus)));
        create(&mut h, 0, None);
        h.feed(0).unwrap();
        h.world.nodes.get_mut(&0).unwrap().source = 1;
        h.record(1, 0);
        let Err(EngineError::Panicked(message)) = h.feed(1) else {
            panic!("a block live in two sources panics the engine thread");
        };
        assert!(message.contains("live in two sources"), "{message}");
        every_stamp_is_fed(&h.clock);
        create(&mut h, 0, None);
        assert_eq!(h.feed(0), Err(EngineError::Panicked(message.clone())));
        every_stamp_is_fed(&h.clock);
        assert_eq!(
            stops(&conditions),
            [EngineError::Panicked(message).to_string()]
        );
    });
}

#[test]
fn a_late_subscriber_gets_the_released_state() {
    within_deadline(|| {
        let mut h = Harness::new();
        h.commit(
            0,
            &Edit::Create {
                parent: None,
                page: true,
            },
        );
        create(&mut h, 1, Some(0));
        h.feed(0).unwrap();
        h.feed(1).unwrap();
        h.commit(0, &Edit::TogglePage(0));
        create(&mut h, 1, Some(1));
        h.feed(1).unwrap();
        h.check_released().unwrap();
        assert_eq!(h.released, 3, "source 0's toggle holds stamp 3");
        let late: Vec<(Released, Receiver<ViewBatch>)> = View::ALL
            .iter()
            .map(|view| {
                let (state, rx) = h.engine.snapshot_and_subscribe(*view).unwrap();
                assert_eq!(state.below.get(), h.released);
                let mut folded = Released::new();
                for (row, diff) in state.deltas {
                    add(&mut folded, row, diff);
                }
                (folded, rx)
            })
            .collect();
        for ((folded, _), view) in late.iter().zip(&h.views) {
            assert!(!folded.is_empty());
            assert_eq!(folded, view);
        }
        h.feed(0).unwrap();
        h.check_released().unwrap();
        assert_eq!(h.released, 5);
        for ((mut folded, rx), view) in late.into_iter().zip(&h.views) {
            for batch in rx.try_iter() {
                for (row, diff) in batch.deltas {
                    add(&mut folded, row, diff);
                }
            }
            assert_eq!(&folded, view);
        }
    });
}

#[test]
fn a_snapshot_names_the_released_version_not_the_watermark() {
    within_deadline(|| {
        let mut h = Harness::new();
        create(&mut h, 1, None);
        create(&mut h, 0, None);
        create(&mut h, 1, None);
        h.feed(1).unwrap();
        h.check_released().unwrap();
        assert_eq!(h.released, 1, "source 1's batch straddles stamp 2");
        assert_eq!(h.clock.low_watermark().get(), 2);
        let (snap, _rx) = h.engine.snapshot_and_subscribe(View::Row).unwrap();
        assert_eq!(
            snap.below.get(),
            h.released,
            "the snapshot names a version the engine never released"
        );
        assert!(snap.deltas.is_empty(), "the snapshot holds unreleased rows");
    });
}

/// The posts of a producer, such as a store's commit callback, must not wait
/// for the engine's work.
#[test]
fn a_feed_returns_while_the_engine_thread_is_held() {
    within_deadline(|| {
        let (held, holding) = mpsc::channel();
        let (go, waiting) = mpsc::channel::<()>();
        let mut h = Harness::with_engine(|clock| {
            ViewEngine::start(clock, move |_| {
                held.send(()).unwrap();
                waiting.recv().unwrap();
            })
        });
        let other = CommitClock::new();
        other.mint(SOURCES[0]);
        h.engine.feed(SOURCES[0], other.mint(SOURCES[0]), vec![]);
        holding.recv().unwrap();
        create(&mut h, 0, None);
        h.engine
            .feed(SOURCES[0], h.stamps[1], vec![(uri(0), h.world.snapshot(0))]);
        h.engine.replace(SOURCES[1], Stamp::NONE, vec![]);
        go.send(()).unwrap();
        h.settle().unwrap_err();
        every_stamp_is_fed(&h.clock);
    });
}

#[test]
#[should_panic(expected = "a clock that minted")]
fn an_engine_refuses_a_clock_that_minted_before_it_started() {
    let clock = Arc::new(CommitClock::new());
    clock.mint(SOURCES[0]);
    ViewEngine::start(clock, |_| {});
}
