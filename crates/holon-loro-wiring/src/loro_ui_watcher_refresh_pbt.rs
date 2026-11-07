//! The live-entity refresh and the block watcher over a real Loro doc re-read
//! the tree exactly when the tree changed: P1, an idle doc costs neither of
//! them a snapshot; P2, after every step
//! the resolver's lookups and the vault profile rows equal a recomputation
//! from a fresh snapshot; P3, after a poll whose version read failed the
//! block shows the error, after every other poll it shows a fresh render.
//!
//! Time is paused on a current-thread runtime, so the poller runs only while
//! the test waits for its next poll and every wait ends at a whole poll.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use holon_api::EntityUri;
use holon_api::UiState;
use holon_api::streaming::WatchHandle;
use holon_api::widget_spec::DataRow;
use holon_core::storage::BlockQuerySource;
use holon_core::storage::BlockSnapshot;
use holon_core::storage::block_query::Result;
use holon_loro_testing::transitions::doc_churn::ChurnContent;
use holon_loro_testing::transitions::doc_churn::DocChurn;
use holon_loro_testing::transitions::doc_churn::DocChurnStep;
use holon_loro_testing::transitions::doc_churn::doc_churn_step;
use proptest::prelude::*;
use proptest::test_runner::Config;
use proptest::test_runner::TestRunner;
use tokio::sync::watch;

use super::turso_free_profile_resolver;
use crate::loro_block_query_source::LoroBlockQuerySource;

const MAX_IDLE_POLLS: usize = 40;

struct CountingSource {
    inner: LoroBlockQuerySource,
    snapshots: AtomicUsize,
    polls: watch::Sender<usize>,
    versions_fail: AtomicBool,
}

fn injected_failure() -> Box<dyn std::error::Error + Send + Sync> {
    "injected version read failure".into()
}

#[async_trait]
impl BlockQuerySource for CountingSource {
    async fn snapshot(&self) -> Result<BlockSnapshot> {
        self.snapshots.fetch_add(1, Ordering::SeqCst);
        self.inner.snapshot().await
    }

    fn change_version(&self) -> Result<Option<u64>> {
        self.polls.send_modify(|n| *n += 1);
        if self.versions_fail.load(Ordering::SeqCst) {
            return Err(injected_failure());
        }
        self.inner.change_version()
    }
}

impl CountingSource {
    fn new(backend: &Arc<holon_loro::loro_backend::LoroBackend>) -> Arc<Self> {
        Arc::new(Self {
            inner: LoroBlockQuerySource::new(Arc::clone(backend)),
            snapshots: AtomicUsize::new(0),
            polls: watch::channel(0).0,
            versions_fail: AtomicBool::new(false),
        })
    }

    fn snapshots(&self) -> usize {
        self.snapshots.load(Ordering::SeqCst)
    }

    fn fail_versions(&self, fail: bool) {
        self.versions_fail.store(fail, Ordering::SeqCst);
    }
}

/// Apply every event the watcher has sent, as a frontend does.
fn drain(watcher: &mut WatchHandle, ui: &mut UiState) {
    loop {
        match watcher.try_recv() {
            Ok(event) => {
                ui.apply_event(event);
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => return,
            Err(e) => panic!("the watcher stopped: {e}"),
        }
    }
}

/// Wait until the poller has started `n` more polls; on a current-thread
/// runtime each of them has also run to its sleep.
async fn wait_polls(polls: &mut watch::Receiver<usize>, n: usize) {
    let target = *polls.borrow_and_update() + n;
    while *polls.borrow_and_update() < target {
        polls.changed().await.expect("the poller's source is alive");
    }
}

type Rows = BTreeMap<String, BTreeMap<String, holon_api::StorageEntity>>;

fn entity_rows(entities: &holon_profiles::LiveEntities) -> Rows {
    entities
        .iter()
        .map(|(name, data)| {
            let rows = data
                .read()
                .iter()
                .map(|(key, row)| (key.clone(), (**row).clone()))
                .collect();
            (name.as_str().to_string(), rows)
        })
        .collect()
}

type Candidates = BTreeMap<String, BTreeMap<String, Vec<holon_api::StorageEntity>>>;

/// Every row a lookup may hold, per entity and parent. Which of two sources
/// under one parent wins is unspecified: the snapshot iterates a hash map.
fn candidate_rows(snapshot: &BlockSnapshot) -> Candidates {
    let mut candidates = Candidates::new();
    for spec in holon_profiles::LiveEntitySpec::ALL {
        let by_parent = candidates
            .entry(spec.entity_name().as_str().to_string())
            .or_default();
        for block in snapshot.iter_blocks() {
            let single = holon_profiles::LiveEntities::from([(
                spec.entity_name(),
                spec.live_data_from_blocks(std::iter::once(block)),
            )]);
            for (parent, row) in entity_rows(&single).into_values().flatten() {
                by_parent.entry(parent).or_default().push(row);
            }
        }
    }
    candidates
}

fn rows_are_candidates(rows: &Rows, candidates: &Candidates) -> bool {
    rows.len() == candidates.len()
        && rows
            .iter()
            .zip(candidates)
            .all(|((name, rows), (cname, by_parent))| {
                name == cname
                    && rows.len() == by_parent.len()
                    && rows
                        .iter()
                        .zip(by_parent)
                        .all(|((parent, row), (cparent, options))| {
                            parent == cparent && options.contains(row)
                        })
            })
}

fn expected_profile_rows(snapshot: &BlockSnapshot) -> BTreeMap<String, String> {
    snapshot
        .iter_blocks()
        .filter(|b| {
            b.content_type == holon_api::ContentType::Source
                && holon_profiles::is_profile_block_by_source_language(
                    b.source_language.as_ref().map(|l| l.to_string()).as_deref(),
                )
        })
        .map(|b| (b.id.as_str().to_string(), b.content.clone()))
        .collect()
}

/// A churn step, and whether the pollers' version reads fail after it.
type FaultyStep = (DocChurnStep, bool);

async fn refresh_follows_the_doc(
    shared_trees: usize,
    steps: &[FaultyStep],
) -> std::result::Result<(), TestCaseError> {
    let mut churn = DocChurn::new(shared_trees).await.expect("a fresh doc");
    let source = CountingSource::new(&churn.backend);
    let mut polls = source.polls.subscribe();
    let watcher_source = CountingSource::new(&churn.backend);
    let mut watcher_polls = watcher_source.polls.subscribe();
    let mut watcher = super::loro_watch_ui(
        Arc::clone(&watcher_source) as Arc<dyn BlockQuerySource>,
        churn.root.clone(),
        holon_advice::AdviceRuleStatusHandle::new(),
    )
    .await
    .expect("the watcher starts");
    let mut ui = UiState::new(super::error_render_expr("no render yet"));
    let fresh_source: Arc<dyn BlockQuerySource> =
        Arc::new(LoroBlockQuerySource::new(Arc::clone(&churn.backend)));
    let shutdown = holon_api::lifecycle::SessionShutdown::new();
    let (resolver, profile_rows) = turso_free_profile_resolver(
        Arc::clone(&source) as Arc<dyn BlockQuerySource>,
        holon_profiles::create_default_registry().expect("default TypeRegistry"),
        &shutdown,
        Arc::new(holon_conditions::ConditionBus::new()),
    );
    wait_polls(&mut polls, 1).await;
    wait_polls(&mut watcher_polls, 1).await;

    for (i, (step, versions_fail)) in steps.iter().enumerate() {
        source.fail_versions(*versions_fail);
        watcher_source.fail_versions(*versions_fail);
        let before = source.inner.snapshot().await.expect("a snapshot");
        let live: Vec<EntityUri> = before
            .iter_blocks()
            .filter(|b| b.id != churn.root)
            .map(|b| b.id.clone())
            .collect();
        churn.apply(step, &live).await.expect("the step applies");
        wait_polls(&mut polls, 1).await;
        wait_polls(&mut watcher_polls, 1).await;
        drain(&mut watcher, &mut ui);

        if *versions_fail {
            prop_assert_eq!(
                ui.render_expr(),
                super::error_render_expr(&format!("{:#}", injected_failure())),
                "P3: the failed version read after step {} {:?} is not rendered",
                i,
                step
            );
        } else {
            let (expr, ordered) = super::render_state(&fresh_source, &churn.root)
                .await
                .expect("a render");
            let rows: HashMap<String, DataRow> = ordered
                .into_iter()
                .map(|(id, row)| {
                    let row = row.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
                    (id.to_string(), row)
                })
                .collect();
            prop_assert_eq!(
                ui.render_expr(),
                expr,
                "P3: the block's render is stale after step {} {:?}",
                i,
                step
            );
            prop_assert_eq!(
                ui.data(),
                &rows,
                "P3: the block's rows are stale after step {} {:?}",
                i,
                step
            );
        }

        let fresh = source.inner.snapshot().await.expect("a snapshot");
        let rows = entity_rows(&resolver.live_entities());
        let candidates = candidate_rows(&fresh);
        prop_assert!(
            rows_are_candidates(&rows, &candidates),
            "P2: the lookups are stale after step {} {:?}: {:?} is not one of {:?}",
            i,
            step,
            rows,
            candidates
        );
        let mirrored: BTreeMap<String, String> = profile_rows
            .read()
            .iter()
            .map(|(id, content)| (id.clone(), (**content).clone()))
            .collect();
        prop_assert_eq!(
            mirrored,
            expected_profile_rows(&fresh),
            "P2: the vault profile rows are stale after step {} {:?}",
            i,
            step
        );

        if let DocChurnStep::Idle { polls: idle } = step {
            let (snapshots, renders) = (source.snapshots(), watcher_source.snapshots());
            wait_polls(&mut polls, *idle).await;
            wait_polls(&mut watcher_polls, *idle).await;
            drain(&mut watcher, &mut ui);
            // A failed read cannot tell an idle doc from a changed one.
            if !versions_fail {
                prop_assert_eq!(
                    source.snapshots() - snapshots,
                    0,
                    "P1: {} idle polls over {} shared trees re-read the tree",
                    idle,
                    shared_trees
                );
            }
            prop_assert_eq!(
                watcher_source.snapshots() - renders,
                0,
                "P1: {} idle polls over {} shared trees re-rendered the block",
                idle,
                shared_trees
            );
        }
    }
    Ok(())
}

fn run(shared_trees: usize, steps: &[FaultyStep]) -> std::result::Result<(), TestCaseError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .expect("a test runtime")
        .block_on(refresh_follows_the_doc(shared_trees, steps))
}

fn text(s: &str) -> ChurnContent {
    ChurnContent::Text(s.to_string())
}

/// Fixed cases, run through the property before the generated ones.
fn fixed_cases() -> Vec<(usize, Vec<FaultyStep>)> {
    let reads_succeed = |steps: Vec<DocChurnStep>| steps.into_iter().map(|s| (s, false)).collect();
    vec![
        // An attached shared tree must not make an idle doc re-read itself.
        (1, reads_succeed(vec![DocChurnStep::Idle { polls: 40 }])),
        // Failed reads over a change and over an idle doc, each followed by a
        // good read with nothing new.
        (
            0,
            vec![
                (
                    DocChurnStep::Create {
                        parent: 0,
                        content: ChurnContent::Source {
                            language: "holon_prql".to_string(),
                            code: "q".to_string(),
                        },
                    },
                    true,
                ),
                (DocChurnStep::Idle { polls: 1 }, false),
                (DocChurnStep::Idle { polls: 1 }, true),
                (DocChurnStep::Idle { polls: 1 }, false),
            ],
        ),
        // A peer's import whose Lamport stamps lie below the doc's own.
        (
            0,
            reads_succeed(vec![
                DocChurnStep::Create {
                    parent: 0,
                    content: text("a"),
                },
                DocChurnStep::Create {
                    parent: 0,
                    content: text("b"),
                },
                DocChurnStep::Create {
                    parent: 0,
                    content: text("c"),
                },
                DocChurnStep::ConcurrentImport {
                    creates: vec![ChurnContent::Source {
                        language: "holon_rule".to_string(),
                        code: "rule".to_string(),
                    }],
                },
            ]),
        ),
        // Two sources under one parent: either may hold the lookup.
        (
            0,
            reads_succeed(vec![
                DocChurnStep::ConcurrentImport {
                    creates: vec![ChurnContent::Source {
                        language: "holon_gql".to_string(),
                        code: String::new(),
                    }],
                },
                DocChurnStep::ConcurrentImport {
                    creates: vec![
                        ChurnContent::Source {
                            language: "holon_prql".to_string(),
                            code: ": :: ".to_string(),
                        },
                        text("  z  "),
                    ],
                },
            ]),
        ),
    ]
}

#[test]
fn the_live_entity_refresh_reads_the_tree_exactly_when_it_changed() {
    let failures: Vec<String> = fixed_cases()
        .into_iter()
        .filter_map(|(shared_trees, steps)| {
            run(shared_trees, &steps)
                .err()
                .map(|e| format!("fixed case ({shared_trees} shared trees, {steps:?}): {e}"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));

    TestRunner::new(Config {
        cases: 64,
        failure_persistence: None,
        ..Config::default()
    })
    .run(
        &(
            0..=2usize,
            proptest::collection::vec(
                (
                    doc_churn_step(MAX_IDLE_POLLS),
                    proptest::bool::weighted(0.25),
                ),
                1..12,
            ),
        ),
        |(shared_trees, steps)| run(shared_trees, &steps),
    )
    .unwrap();
}
