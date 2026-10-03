//! Two writers commit to the global and the layout doc while the projection
//! runs. Every version the view engine releases equals the views recomputed
//! from both docs at that version: each doc at its last commit stamped below
//! the version.
//!
//! @pbt kind harness
//! @pbt covers engine-version-law-over-loro — a feed whose rows hold a commit
//!   after its cover (forced at the read seam), a feed of an unsettled read,
//!   and a lost feed, each seen as a released version that differs from the
//!   authority

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use holon_api::commit_clock::CommitClock;
use holon_api::commit_clock::CommitSource;
use holon_api::commit_clock::Stamp;
use holon_core::OriginTaggedWrites;
use holon_integration_tests::pbt::engine_views::FoldedViews;
use holon_integration_tests::pbt::engine_views::only_in;
use holon_integration_tests::pbt::engine_views::recompute;
use holon_integration_tests::pbt::engine_views::start_views_engine;
use holon_loro::CONTENT_RAW;
use holon_loro::DocScope;
use holon_loro::LoroDocument;
use holon_loro::LoroDocumentStore;
use holon_loro::LoroProjection;
use holon_loro::SinkReader;
use holon_loro::TREE_NAME;
use holon_loro::WriteOrigin;
use holon_loro::snapshot_blocks_from_doc;
use holon_views::views::View;
use loro::Frontiers;
use loro::TreeID;
use tokio::sync::RwLock;

use crate::projection_harness::MemorySink;

const WRITES: usize = 80;
const DEADLINE: Duration = Duration::from_secs(30);
const SEAM_WAIT: Duration = Duration::from_millis(200);

/// One writer's doc: the only thread that commits to it.
struct Writer {
    doc: Arc<LoroDocument>,
    name: &'static str,
    /// The stamp of each commit, in commit order, recorded under the doc's
    /// write guard.
    minted: Arc<StdMutex<Vec<Stamp>>>,
    _subscription: loro::Subscription,
}

impl Writer {
    /// Subscribed after the projection, so its callback runs after the mint.
    fn new(
        doc: Arc<LoroDocument>,
        source: CommitSource,
        name: &'static str,
        clock: &Arc<CommitClock>,
    ) -> Writer {
        let minted = Arc::new(StdMutex::new(Vec::<Stamp>::new()));
        let (record, clock) = (minted.clone(), clock.clone());
        // ALLOW(loro_doc_escape): subscription registration only; the callback
        // never reads doc state.
        let subscription = doc.doc().subscribe_root(Arc::new(move |_| {
            let stamp = *clock
                .outstanding(source)
                .last()
                .expect("the projection's callback minted this commit's stamp before this one ran");
            let mut record = record.lock().unwrap();
            assert!(
                record.last() < Some(&stamp),
                "{source:?}: stamp {stamp:?} is not this commit's own (last recorded {:?})",
                record.last()
            );
            record.push(stamp);
        }));
        Writer {
            doc,
            name,
            minted,
            _subscription: subscription,
        }
    }

    /// Commits `WRITES` edits; returns each commit's stamp with the doc's
    /// frontier after it.
    fn run(&self, seed: u64) -> Result<Vec<(Stamp, Frontiers)>> {
        let mut rng = seed;
        let mut next = move |n: usize| {
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (rng >> 33) as usize % n.max(1)
        };
        let mut parents: BTreeMap<TreeID, Option<TreeID>> = BTreeMap::new();
        let mut history = Vec::new();
        for i in 0..WRITES {
            let nodes: Vec<TreeID> = parents.keys().copied().collect();
            let leaves: Vec<TreeID> = nodes
                .iter()
                .copied()
                .filter(|n| !parents.values().any(|p| *p == Some(*n)))
                .collect();
            let pick = |list: &[TreeID], k: usize| list.get(k).copied();
            let op = if nodes.is_empty() { 0 } else { next(5) };
            let (a, b) = (next(nodes.len()), next(nodes.len() + 1));
            self.doc
                .with_write(WriteOrigin::Probe("engine_version_law"), |txn| {
                    let tree = txn.get_tree(TREE_NAME);
                    match op {
                        0 => {
                            let parent = pick(&nodes, b);
                            let node = tree.create(parent)?;
                            holon_loro::write_stable_id(txn, node, &format!("{}-{i}", self.name))?;
                            tree.get_meta(node)?
                                .ensure_mergeable_text(CONTENT_RAW)?
                                .insert(0, "x")?;
                            parents.insert(node, parent);
                        }
                        1 => {
                            tree.get_meta(nodes[a])?
                                .ensure_mergeable_text(CONTENT_RAW)?
                                .insert(0, "e")?;
                        }
                        2 => {
                            let meta = tree.get_meta(nodes[a])?;
                            if meta.get("tags").is_some() {
                                meta.delete("tags")?;
                            } else {
                                meta.insert("tags", r#"["Page"]"#)?;
                            }
                        }
                        3 => {
                            let leaf = leaves[a % leaves.len()];
                            let parent = pick(&nodes, b).filter(|p| *p != leaf);
                            if parents[&leaf] == parent {
                                tree.get_meta(leaf)?
                                    .ensure_mergeable_text(CONTENT_RAW)?
                                    .insert(0, "m")?;
                            } else {
                                tree.mov(leaf, parent)?;
                                parents.insert(leaf, parent);
                            }
                        }
                        _ => {
                            let leaf = leaves[a % leaves.len()];
                            tree.delete(leaf)?;
                            parents.remove(&leaf);
                        }
                    }
                    Ok(())
                })?;
            self.record(&mut history)?;
        }
        Ok(history)
    }

    /// Appends the stamp and the frontier of the doc's last commit, which
    /// must be a commit after the last one in `history`.
    fn record(&self, history: &mut Vec<(Stamp, Frontiers)>) -> Result<()> {
        let stamp = *self
            .minted
            .lock()
            .unwrap()
            .last()
            .expect("each commit is stamped");
        assert!(
            history.last().is_none_or(|(last, _)| *last < stamp),
            "{}: the commit after {:?} minted no stamp",
            self.name,
            history.last().map(|(stamp, _)| stamp)
        );
        history.push((stamp, self.doc.with_read(|d| Ok(d.oplog_frontiers()))?));
        Ok(())
    }

    /// The doc's blocks after its last commit stamped below `below`.
    fn blocks_below(
        &self,
        history: &[(Stamp, Frontiers)],
        below: Stamp,
    ) -> Result<Vec<holon_loro::SnapshotBlock>> {
        let at = history
            .iter()
            .take_while(|(stamp, _)| *stamp < below)
            .last()
            .map(|(_, frontier)| frontier.clone())
            .unwrap_or_default();
        let fork = self.doc.with_read(|d| Ok(d.fork_at(&at)?))?;
        Ok(snapshot_blocks_from_doc(&fork).into_values().collect())
    }
}

/// A projection over two fresh docs, armed, with a writer on each doc.
struct Setup {
    _tempdir: tempfile::TempDir,
    clock: Arc<CommitClock>,
    folded: FoldedViews,
    projection: Arc<LoroProjection>,
    writers: Arc<[Writer; 2]>,
}

impl Setup {
    async fn new() -> Result<Setup> {
        let tempdir = tempfile::tempdir()?;
        let doc_store = Arc::new(RwLock::new(LoroDocumentStore::new(
            tempdir.path().to_path_buf(),
        )));
        let global = doc_store.read().await.get_doc(DocScope::Global).await?;
        let layout = doc_store.read().await.get_doc(DocScope::Layout).await?;
        let sink = Arc::new(MemorySink::new());
        let (clock, engine) = start_views_engine();
        let folded = FoldedViews::subscribe(&engine)?;
        let projection = Arc::new(LoroProjection::new(
            doc_store.clone(),
            Arc::new(StdMutex::new(Frontiers::default())),
            sink.clone() as Arc<dyn OriginTaggedWrites>,
            sink.clone() as Arc<dyn SinkReader>,
            tempdir.path().join("sidecar").join("sc.sync"),
            holon_api::block_read_model::BlockReadModel::new(),
            Arc::new(holon_api::ConditionBus::new()),
            clock.clone(),
            engine,
        ));
        projection.install_doc_subscriptions().await?;
        projection.arm();
        let writers = Arc::new([
            Writer::new(global, CommitSource::LoroGlobal, "g", &clock),
            Writer::new(layout, CommitSource::LoroLayout, "l", &clock),
        ]);
        Ok(Setup {
            _tempdir: tempdir,
            clock,
            folded,
            projection,
            writers,
        })
    }

    /// Projects until settled, then checks every version released up to the
    /// last commit against both docs at that version.
    async fn assert_versions_equal_the_authority(
        &mut self,
        histories: &[Vec<(Stamp, Frontiers)>; 2],
    ) -> Result<()> {
        let deadline = Instant::now() + DEADLINE;
        let heads = |doc: &LoroDocument| doc.with_read(|d| Ok(d.oplog_frontiers()));
        while !self
            .projection
            .is_settled_at(&heads(&self.writers[0].doc)?, &heads(&self.writers[1].doc)?)
        {
            assert!(Instant::now() < deadline, "the projection did not settle");
            self.projection.project().await?;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let last = self.clock.high_water();
        let mut checked = 0;
        for (i, view) in View::ALL.into_iter().enumerate() {
            while self.folded.below[i] <= last {
                let Some(below) = self.folded.next(i) else {
                    assert!(
                        Instant::now() < deadline,
                        "{view:?}: no version released past {:?}; the last commit is {last:?}",
                        self.folded.below[i]
                    );
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    continue;
                };
                let mut blocks = self.writers[0].blocks_below(&histories[0], below)?;
                blocks.extend(self.writers[1].blocks_below(&histories[1], below)?);
                let expected = &recompute(&blocks, &[])?[i];
                assert!(
                    &self.folded.states[i] == expected,
                    "{view:?} at the version below {below:?} differs from the authority\n  \
                     engine only: {:?}\n  authority only: {:?}",
                    only_in(&self.folded.states[i], expected, 5),
                    only_in(expected, &self.folded.states[i], 5),
                );
                checked += 1;
            }
        }
        assert!(checked >= View::ALL.len(), "no version was checked");
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loro_engine_versions_equal_the_authority() -> Result<()> {
    let mut setup = Setup::new().await?;
    let threads: Vec<_> = (0..2)
        .map(|w| {
            let writers = setup.writers.clone();
            std::thread::spawn(move || writers[w].run(w as u64 + 7))
        })
        .collect();
    while !threads.iter().all(|t| t.is_finished()) {
        setup.projection.project().await?;
        tokio::task::yield_now().await;
    }
    let mut histories = threads.into_iter().map(|t| t.join().unwrap());
    let histories = [histories.next().unwrap()?, histories.next().unwrap()?];
    setup.assert_versions_equal_the_authority(&histories).await
}

/// A commit that lands after a read took its doc's cover and before it read
/// the rows. The rows must not hold it: a released version below its stamp
/// would show it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_commit_at_the_read_seam_is_not_in_the_version_below_it() -> Result<()> {
    let mut setup = Setup::new().await?;
    let global = setup.writers[0].doc.clone();
    let edit = |doc: &LoroDocument, node: TreeID, text: &'static str| {
        doc.with_write(WriteOrigin::Probe("engine_read_seam"), |txn| {
            txn.get_tree(TREE_NAME)
                .get_meta(node)?
                .ensure_mergeable_text(CONTENT_RAW)?
                .insert(0, text)?;
            Ok(())
        })
    };
    let create = |name: &'static str| {
        global.with_write(WriteOrigin::Probe("engine_read_seam"), |txn| {
            let node = txn.get_tree(TREE_NAME).create(None)?;
            holon_loro::write_stable_id(txn, node, name)?;
            txn.get_tree(TREE_NAME)
                .get_meta(node)?
                .ensure_mergeable_text(CONTENT_RAW)?
                .insert(0, name)?;
            Ok(node)
        })
    };
    let mut history = Vec::new();
    let a = create("a")?;
    setup.writers[0].record(&mut history)?;
    setup.projection.project().await?;
    let b = create("b")?;
    setup.writers[0].record(&mut history)?;
    edit(&global, a, "x")?;
    setup.writers[0].record(&mut history)?;

    let racer: Arc<StdMutex<Option<std::thread::JoinHandle<Result<()>>>>> = Arc::default();
    let fired = AtomicBool::new(false);
    let seam_racer = racer.clone();
    let seam_doc = global.clone();
    setup.projection.set_read_seam(Arc::new(move |source| {
        if source != CommitSource::LoroGlobal || fired.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut racer = seam_racer.lock().unwrap();
        let (done, landed) = std::sync::mpsc::channel();
        let doc = seam_doc.clone();
        *racer = Some(std::thread::spawn(move || {
            edit(&doc, b, "y")?;
            // Refused when the edit lands after the read: the seam no longer waits.
            done.send(()).ok();
            Ok(())
        }));
        // Under the read guard the edit cannot land; past it, it lands well
        // within this wait.
        if let Err(RecvTimeoutError::Disconnected) = landed.recv_timeout(SEAM_WAIT) {
            panic!("the racing edit failed; its thread's result names the error");
        }
    }));
    setup.projection.project().await?;
    let racer = racer
        .lock()
        .unwrap()
        .take()
        .expect("the read reached the seam");
    racer.join().unwrap()?;
    setup.writers[0].record(&mut history)?;

    setup
        .assert_versions_equal_the_authority(&[history, Vec::new()])
        .await
}

/// Each doc holds a block before the projection subscribes to it, as a doc
/// loaded from disk does. No commit after that is needed for the engine to
/// release them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_rows_a_doc_held_before_the_projection_are_released() -> Result<()> {
    let tempdir = tempfile::tempdir()?;
    let doc_store = Arc::new(RwLock::new(LoroDocumentStore::new(
        tempdir.path().to_path_buf(),
    )));
    let docs = [
        doc_store.read().await.get_doc(DocScope::Global).await?,
        doc_store.read().await.get_doc(DocScope::Layout).await?,
    ];
    for (doc, name) in docs.iter().zip(["g", "l"]) {
        doc.with_write(WriteOrigin::Probe("engine_loaded_doc"), |txn| {
            let node = txn.get_tree(TREE_NAME).create(None)?;
            holon_loro::write_stable_id(txn, node, name)?;
            Ok(())
        })?;
    }
    let sink = Arc::new(MemorySink::new());
    let (clock, engine) = start_views_engine();
    let projection = LoroProjection::new(
        doc_store.clone(),
        Arc::new(StdMutex::new(Frontiers::default())),
        sink.clone() as Arc<dyn OriginTaggedWrites>,
        sink.clone() as Arc<dyn SinkReader>,
        tempdir.path().join("sidecar").join("sc.sync"),
        holon_api::block_read_model::BlockReadModel::new(),
        Arc::new(holon_api::ConditionBus::new()),
        clock,
        engine.clone(),
    );
    projection.install_doc_subscriptions().await?;
    projection.arm();
    let deadline = Instant::now() + DEADLINE;
    let heads = |doc: &LoroDocument| doc.with_read(|d| Ok(d.oplog_frontiers()));
    while !projection.is_settled_at(&heads(&docs[0])?, &heads(&docs[1])?) {
        assert!(Instant::now() < deadline, "the projection did not settle");
        projection.project().await?;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let mut blocks = Vec::new();
    for doc in &docs {
        blocks.extend(
            doc.with_read(|d| Ok(snapshot_blocks_from_doc(d)))?
                .into_values(),
        );
    }
    assert_eq!(blocks.len(), 2);
    let expected = recompute(&blocks, &[])?;
    let released = FoldedViews::subscribe(&engine)?;
    for (i, view) in View::ALL.into_iter().enumerate() {
        assert!(
            released.states[i] == expected[i],
            "{view:?} released below {:?} differs from the docs\n  engine only: {:?}\n  \
             authority only: {:?}",
            released.below[i],
            only_in(&released.states[i], &expected[i], 5),
            only_in(&expected[i], &released.states[i], 5),
        );
    }
    Ok(())
}
