//! Shared projection convergence wait — the signal-level core behind both the
//! composed per-transition settle (`wide_e2e::converge_projections`) and the
//! `HeadlessFrontendComponent` boot settle. One implementation so the two
//! settles can never drift apart on which projections count as "drained".

use std::sync::Arc;
use std::time::Duration;

use holon::api::BackendEngine;
use holon::sync::matview_manager::LaggingSubscription;
use holon_filesystem::InMemoryFileSystem;
use holon_filesystem::change_source::FileChange;
use holon_filesystem::change_source::FileChangeSource;
use holon_filesystem::writeback_net;
use holon_loro::LoroSyncControllerHandle;
use holon_net::exec::WhyNot;
use holon_orgmode::OrgSyncIdleSignal;

/// The SQL of the block matview the `BlockFeed` mirrors.
const BLOCK_FEED_SQL: &str = "SELECT * FROM block";

/// When [`converge_signals`] gives up.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Patience {
    /// A fixed wait from the start.
    Budget(Duration),
    /// As long as the SUT keeps doing work: give up after `stall` in which no
    /// stage and no work counter moved, or at `ceiling` regardless.
    WhileProgressing { stall: Duration, ceiling: Duration },
}

/// Why [`converge_signals`] gave up, and the stages still short of their
/// targets when it did.
#[derive(Clone, Debug)]
pub(crate) struct GaveUp {
    pub(crate) why: GaveUpWhy,
    pub(crate) owed: Vec<Owed>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum GaveUpWhy {
    Budget(Duration),
    Stalled { stall: Duration, waited: Duration },
    Ceiling(Duration),
}

impl std::fmt::Display for GaveUp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.why {
            GaveUpWhy::Budget(budget) => write!(f, "within {budget:?}")?,
            GaveUpWhy::Stalled { stall, waited } => write!(
                f,
                "after {waited:?}: no stage and no work counter moved for {stall:?}"
            )?,
            GaveUpWhy::Ceiling(ceiling) => write!(
                f,
                "within the {ceiling:?} ceiling, although work kept advancing"
            )?,
        }
        for owed in &self.owed {
            write!(f, "; {owed}")?;
        }
        Ok(())
    }
}

/// One stage short of its target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Owed {
    pub(crate) stage: &'static str,
    pub(crate) target: String,
    pub(crate) value: String,
}

impl std::fmt::Display for Owed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "stage {} at {}, target {}",
            self.stage, self.value, self.target
        )
    }
}

/// Every stage's position against its target, read at one instant. `None`
/// for a stage this draw does not wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Progress {
    pub(crate) loro_settled: Option<bool>,
    pub(crate) cdc: Option<CdcProgress>,
    pub(crate) block_feed: Option<FeedProgress>,
    pub(crate) org: Option<OrgProgress>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CdcProgress {
    /// `cdc_emitted_watermark` — the target every CDC stage is judged by.
    pub(crate) emitted: u64,
    pub(crate) routed_through: u64,
    pub(crate) lagging: Vec<LaggingSubscription>,
    pub(crate) watch_releases_in_flight: u64,
    pub(crate) watch_registrations_in_flight: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FeedProgress {
    /// The last batch the demux routed to the feed's subscription.
    pub(crate) routed: u64,
    pub(crate) consumed: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OrgProgress {
    /// The in-memory filesystem's last change seq.
    pub(crate) changed: u64,
    pub(crate) processed: u64,
    pub(crate) writeback_unsettled: Vec<String>,
}

/// The stages of `progress` short of their targets; empty = settled.
pub(crate) fn owed(progress: &Progress) -> Vec<Owed> {
    let mut owed = Vec::new();
    if progress.loro_settled == Some(false) {
        owed.push(Owed {
            stage: "loro",
            target: "the authority's oplog frontiers".into(),
            value: "an older synced frontier".into(),
        });
    }
    if let Some(cdc) = &progress.cdc {
        if cdc.routed_through < cdc.emitted {
            owed.push(Owed {
                stage: "cdc-routed",
                target: format!("emitted seq {}", cdc.emitted),
                value: format!("routed seq {}", cdc.routed_through),
            });
        }
        for lag in &cdc.lagging {
            owed.push(Owed {
                stage: "cdc-applied",
                target: format!("sent seq {} to view {}", lag.sent, lag.view_name),
                value: format!("applied seq {}", lag.applied),
            });
        }
        if cdc.watch_releases_in_flight > 0 {
            owed.push(Owed {
                stage: "watch-release",
                target: "0 releases in flight".into(),
                value: format!("{} in flight", cdc.watch_releases_in_flight),
            });
        }
        if cdc.watch_registrations_in_flight > 0 {
            owed.push(Owed {
                stage: "watch-register",
                target: "0 registrations in flight".into(),
                value: format!("{} in flight", cdc.watch_registrations_in_flight),
            });
        }
    }
    if let Some(feed) = &progress.block_feed
        && feed.consumed < feed.routed
    {
        owed.push(Owed {
            stage: "block-feed",
            target: format!("routed seq {}", feed.routed),
            value: format!("consumed seq {}", feed.consumed),
        });
    }
    if let Some(org) = &progress.org {
        if org.processed < org.changed {
            owed.push(Owed {
                stage: "org-ingest",
                target: format!("file change seq {}", org.changed),
                value: format!("processed seq {}", org.processed),
            });
        }
        if !org.writeback_unsettled.is_empty() {
            owed.push(Owed {
                stage: "org-writeback",
                target: "nothing unsettled".into(),
                value: org.writeback_unsettled.join(", "),
            });
        }
    }
    owed
}

/// The filesystem an org sync loop watches. Its loop sees only changes made
/// after it subscribed; the session's initial scan covers `scanned_through`.
pub(crate) struct OrgFiles<'a> {
    pub(crate) emitted: &'a EmittedChanges,
    pub(crate) scanned_through: u64,
}

/// The highest change seq the filesystem broadcast to its watchers. A refused
/// write-back ticks the clock without a broadcast, so the clock overstates it.
pub(crate) struct EmittedChanges {
    changes: std::sync::Mutex<tokio::sync::broadcast::Receiver<FileChange>>,
    through: std::sync::atomic::AtomicU64,
}

impl EmittedChanges {
    pub(crate) fn watch(fs: &InMemoryFileSystem) -> Self {
        let changes = FileChangeSource::subscribe(fs);
        Self {
            changes: std::sync::Mutex::new(changes),
            through: std::sync::atomic::AtomicU64::new(fs.last_change_seq()),
        }
    }

    pub(crate) fn through(&self) -> u64 {
        use tokio::sync::broadcast::error::TryRecvError;
        let mut changes = self.changes.lock().expect("emitted changes poisoned");
        loop {
            match changes.try_recv() {
                Ok(change) => {
                    self.through
                        .fetch_max(change.seq, std::sync::atomic::Ordering::Relaxed);
                }
                // The retained changes are the newest, so their max covers the skipped ones.
                Err(TryRecvError::Lagged(_)) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Closed) => {
                    panic!("the in-memory filesystem dropped its change channel")
                }
            }
        }
        self.through.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// What the write-back still owes. A catch-up that waits out its backoff after
/// a refused write is the disclosed steady state of that file, not owed work.
fn writeback_owed(idle: &OrgSyncIdleSignal) -> Vec<String> {
    let mut owed = Vec::new();
    if idle.pass_in_flight() {
        owed.push("a write-back pass is running or a bulk re-render is owed".to_string());
    }
    if idle.writeback_fold_in_flight() {
        owed.push("the home_by fold is running".to_string());
    }
    let queued = idle.queued_renders().documents();
    if !queued.is_empty() {
        owed.push(format!("re-renders of {} are queued", queued.join(", ")));
    }
    let behind = idle.writeback_net().read(|exec| {
        exec.tokens()
            .subjects(writeback_net::BASE)
            .into_iter()
            .filter(|doc| {
                exec.tokens()
                    .token(writeback_net::DISK_CHANGED, doc)
                    .is_some()
                    || writeback_net::why_not(exec, writeback_net::READ_DISK, doc).is_none()
                    || writeback_net::CATCH_UP.iter().any(|t| {
                        matches!(
                            writeback_net::why_not(exec, t, doc),
                            None | Some(WhyNot::InFlight)
                        )
                    })
            })
            .map(|doc| doc.to_string())
            .collect::<Vec<_>>()
    });
    if !behind.is_empty() {
        owed.push(format!("{} behind its store", behind.join(", ")));
    }
    owed
}

/// The signals a settle reads. An absent one is a stage this draw does not
/// wire, so there is nothing to wait for.
pub(crate) struct Signals<'a> {
    pub(crate) engine: Option<&'a Arc<BackendEngine>>,
    pub(crate) sync: Option<Arc<LoroSyncControllerHandle>>,
    pub(crate) org_idle: Option<Arc<OrgSyncIdleSignal>>,
    pub(crate) org_files: Option<OrgFiles<'a>>,
    pub(crate) block_feed: Option<Arc<holon_api::live_data::LiveData<holon_api::Block>>>,
}

impl Signals<'_> {
    async fn progress(&self) -> Progress {
        let loro_settled = match &self.sync {
            Some(sync) => Some(
                sync.is_settled()
                    .await
                    .expect("converge_signals: Loro settle predicate failed"),
            ),
            None => None,
        };
        let cdc = self.engine.map(|engine| {
            let emitted = engine.db_handle().cdc_emitted_watermark();
            let settled = engine.matview_manager().settled_up_to();
            CdcProgress {
                emitted,
                routed_through: settled.routed_through,
                lagging: settled.lagging,
                watch_releases_in_flight: engine.watch_releases_in_flight(),
                watch_registrations_in_flight: engine.watch_registrations_in_flight(),
            }
        });
        let block_feed = self.block_feed.as_ref().map(|feed| {
            let engine = self
                .engine
                .expect("converge_signals: a block feed is a CDC consumer, so it needs the engine");
            let view = holon::sync::MatviewManager::compute_view_name(BLOCK_FEED_SQL);
            FeedProgress {
                routed: engine.matview_manager().last_routed(&view),
                consumed: feed.consumed_seq(),
            }
        });
        let org = self.org_idle.as_ref().map(|idle| {
            let files = self
                .org_files
                .as_ref()
                .expect("converge_signals: an org sync loop reads the in-memory filesystem");
            OrgProgress {
                changed: files.emitted.through(),
                processed: idle.processed_change_seq().max(files.scanned_through),
                writeback_unsettled: writeback_owed(idle),
            }
        });
        Progress {
            loro_settled,
            cdc,
            block_feed,
            org,
        }
    }
}

/// Wait until every stage reached its target: Loro synced to the authority's
/// frontiers, every CDC batch up to the emitted watermark routed and applied
/// by each reporting subscriber, the block feed caught up with what was
/// routed to it, and org ingest and write-back caught up with the
/// filesystem. Settled = two consecutive reads with nothing owed and nothing
/// moved, so work that a stage started between the reads is not missed.
///
/// `patience` only guards against a hang; `Err` names each stage still short
/// of its target, and the caller MUST decide (the composed settle fails loud
/// on it).
pub(crate) async fn converge_signals(
    signals: Signals<'_>,
    patience: Patience,
) -> Result<(), GaveUp> {
    let started = tokio::time::Instant::now();
    let poll = Duration::from_millis(2);
    let work = || {
        (
            holon_filesystem::ingest_progress::snapshot(),
            holon_loro::projection_stats::snapshot(),
            signals.sync.as_ref().map(|s| s.last_synced_frontiers()),
        )
    };
    let mut last_work = work();
    let mut last_progress_at = started;
    let mut last = signals.progress().await;
    let mut polls_owed_by_stage = std::collections::BTreeMap::<&str, u32>::new();

    loop {
        tokio::time::sleep(poll).await;
        let now_progress = signals.progress().await;
        let owing = owed(&now_progress);
        if owing.is_empty() && now_progress == last {
            tracing::debug!(target: "holon_settle", ?polls_owed_by_stage, "stages waited for");
            return Ok(());
        }
        for o in &owing {
            *polls_owed_by_stage.entry(o.stage).or_default() += 1;
        }

        let now = tokio::time::Instant::now();
        let now_work = work();
        if now_progress != last || now_work != last_work {
            last_work = now_work;
            last_progress_at = now;
        }
        last = now_progress;

        let waited = now.duration_since(started);
        let why = match patience {
            Patience::Budget(budget) if waited >= budget => Some(GaveUpWhy::Budget(budget)),
            Patience::WhileProgressing { ceiling, .. } if waited >= ceiling => {
                Some(GaveUpWhy::Ceiling(ceiling))
            }
            Patience::WhileProgressing { stall, .. }
                if now.duration_since(last_progress_at) >= stall =>
            {
                Some(GaveUpWhy::Stalled { stall, waited })
            }
            _ => None,
        };
        if let Some(why) = why {
            return Err(GaveUp { why, owed: owing });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;

    use holon_api::live_data::DeliveryCursor;
    use holon_api::live_data::reporting_to;
    use proptest::prelude::*;
    use tokio_stream::StreamExt;

    use super::*;

    const STAGES: [&str; 8] = [
        "loro",
        "cdc-routed",
        "cdc-applied",
        "watch-release",
        "watch-register",
        "block-feed",
        "org-ingest",
        "org-writeback",
    ];

    /// Every stage wired and at its target.
    fn settled() -> impl Strategy<Value = Progress> {
        (0u64..1_000, 0u64..3, 0u64..1_000, 0u64..1_000).prop_map(
            |(emitted, ahead, routed, changed)| Progress {
                loro_settled: Some(true),
                cdc: Some(CdcProgress {
                    emitted,
                    routed_through: emitted + ahead,
                    lagging: vec![],
                    watch_releases_in_flight: 0,
                    watch_registrations_in_flight: 0,
                }),
                block_feed: Some(FeedProgress {
                    routed,
                    consumed: routed,
                }),
                org: Some(OrgProgress {
                    changed,
                    processed: changed,
                    writeback_unsettled: vec![],
                }),
            },
        )
    }

    /// `p` with only `stage` put `by` short of its target.
    fn behind(mut p: Progress, stage: &str, by: u64) -> Progress {
        let cdc = p.cdc.as_mut().expect("settled() wires cdc");
        let feed = p.block_feed.as_mut().expect("settled() wires the feed");
        let org = p.org.as_mut().expect("settled() wires org");
        match stage {
            "loro" => p.loro_settled = Some(false),
            "cdc-routed" => {
                cdc.emitted += by;
                cdc.routed_through = cdc.emitted - by;
            }
            "cdc-applied" => cdc.lagging.push(LaggingSubscription {
                view_name: "watch_view_x".into(),
                sent: cdc.emitted,
                applied: cdc.emitted.saturating_sub(by),
            }),
            "watch-release" => cdc.watch_releases_in_flight = by,
            "watch-register" => cdc.watch_registrations_in_flight = by,
            "block-feed" => feed.routed += by,
            "org-ingest" => org.changed += by,
            "org-writeback" => org.writeback_unsettled.push("pending render".into()),
            other => unreachable!("unknown stage {other}"),
        }
        p
    }

    proptest! {
        /// A settle can only end when nothing is owed, so no single stage
        /// short of its target may go unnamed, whatever the others do.
        #[test]
        fn one_stage_behind_is_owed_by_name(
            p in settled(),
            stage in proptest::sample::select(&STAGES[..]),
            by in 1u64..100,
        ) {
            prop_assert_eq!(owed(&p), vec![]);
            let owing = owed(&behind(p, stage, by));
            prop_assert_eq!(owing.len(), 1, "{:?}", owing);
            prop_assert_eq!(owing[0].stage, stage);
        }
    }

    #[test]
    fn a_stage_left_unwired_owes_nothing() {
        let unwired = Progress {
            loro_settled: None,
            cdc: None,
            block_feed: None,
            org: None,
        };
        assert_eq!(owed(&unwired), vec![]);
    }

    fn engine_signals(engine: &Arc<BackendEngine>) -> Signals<'_> {
        Signals {
            engine: Some(engine),
            sync: None,
            org_idle: None,
            org_files: None,
            block_feed: None,
        }
    }

    /// A keyed watch whose consumer applies each batch only after
    /// `apply_delay`, or never when `None`. Returns the cursor and the
    /// count of applied batches.
    async fn slow_keyed_watch(
        engine: &Arc<BackendEngine>,
        apply_delay: Option<Duration>,
    ) -> (Arc<DeliveryCursor>, Arc<AtomicU64>) {
        create_probe_table(engine).await;
        let cursor = Arc::new(DeliveryCursor::default());
        let (_view, mut stream) = reporting_to(
            cursor.clone(),
            engine.matview_manager().ensure_and_subscribe(
                "SELECT id, owner AS watch_key FROM settle_probe",
                Some("mine"),
            ),
        )
        .await
        .expect("watch the probe view");
        let applied = Arc::new(AtomicU64::new(0));
        let (consumer_cursor, consumer_applied) = (cursor.clone(), applied.clone());
        tokio::spawn(async move {
            while let Some(batch) = stream.next().await {
                match apply_delay {
                    Some(delay) => tokio::time::sleep(delay).await,
                    None => std::future::pending().await,
                }
                consumer_applied.fetch_add(1, Ordering::SeqCst);
                consumer_cursor.applied_through(batch.metadata.seq);
            }
        });
        (cursor, applied)
    }

    async fn create_probe_table(engine: &Arc<BackendEngine>) {
        engine
            .db_handle()
            .execute_ddl("CREATE TABLE settle_probe (id TEXT PRIMARY KEY, owner TEXT NOT NULL)")
            .await
            .expect("create the probe table");
    }

    async fn insert(engine: &Arc<BackendEngine>, id: &str, owner: &str) {
        engine
            .db_handle()
            .execute(
                &format!("INSERT INTO settle_probe (id, owner) VALUES ('{id}', '{owner}')"),
                vec![],
            )
            .await
            .expect("insert a probe row");
    }

    /// The settle waits for a slow subscriber to apply its batch, and a batch
    /// with nothing for that subscriber neither holds the settle nor leaves the
    /// subscriber's cursor behind.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_settle_waits_for_a_slow_subscriber_to_apply() {
        let engine = holon::di::test_helpers::create_test_engine()
            .await
            .expect("test engine");
        let (cursor, applied) = slow_keyed_watch(&engine, Some(Duration::from_millis(400))).await;

        insert(&engine, "a", "mine").await;
        converge_signals(
            engine_signals(&engine),
            Patience::Budget(Duration::from_secs(30)),
        )
        .await
        .unwrap_or_else(|gave_up| panic!("settle gave up {gave_up}"));
        assert_eq!(
            applied.load(Ordering::SeqCst),
            1,
            "the settle returned before the subscriber applied the batch routed to it"
        );

        let before = cursor.applied();
        insert(&engine, "b", "someone else").await;
        converge_signals(
            engine_signals(&engine),
            Patience::Budget(Duration::from_secs(30)),
        )
        .await
        .unwrap_or_else(|gave_up| panic!("settle gave up {gave_up}"));
        assert_eq!(
            applied.load(Ordering::SeqCst),
            1,
            "the other owner's row reached this watch"
        );
        assert!(
            cursor.applied() > before && !cursor.is_lagging(),
            "a batch filtered to empty left the caught-up cursor at {before} (now sent {}, applied {})",
            cursor.sent(),
            cursor.applied()
        );
    }

    /// A watch whose consumer has gone owes no apply. Its enriching stream
    /// still accepts the next batch and only closes behind it, so the
    /// subscription stays registered with that batch routed and unapplied.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_watch_whose_consumer_left_does_not_hold_the_settle() {
        let engine = holon::di::test_helpers::create_test_engine()
            .await
            .expect("test engine");
        create_probe_table(&engine).await;
        for i in 0..50 {
            insert(&engine, &format!("before-{i}"), "mine").await;
        }
        let cursor = Arc::new(DeliveryCursor::default());
        let (_view, raw) = reporting_to(
            cursor.clone(),
            engine
                .matview_manager()
                .ensure_and_subscribe("SELECT id, owner FROM settle_probe", None),
        )
        .await
        .expect("watch the probe view");
        drop(holon::api::ui_watcher::enrich_stream(
            raw,
            engine.profile_resolver().clone(),
            Default::default(),
        ));
        drop(cursor);

        insert(&engine, "after", "mine").await;
        converge_signals(
            engine_signals(&engine),
            Patience::Budget(Duration::from_secs(5)),
        )
        .await
        .unwrap_or_else(|gave_up| panic!("settle gave up {gave_up}"));
    }

    /// A deadline that fires names the stage, its target and where it stands.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_deadline_names_the_stage_its_target_and_its_value() {
        let engine = holon::di::test_helpers::create_test_engine()
            .await
            .expect("test engine");
        let (cursor, _) = slow_keyed_watch(&engine, None).await;
        let applied_before = cursor.applied();

        insert(&engine, "a", "mine").await;
        let gave_up = converge_signals(
            engine_signals(&engine),
            Patience::Budget(Duration::from_millis(300)),
        )
        .await
        .expect_err("a subscriber that never applies cannot settle");

        let message = gave_up.to_string();
        let expected = format!(
            "stage cdc-applied at applied seq {applied_before}, target sent seq {} to view ",
            cursor.sent()
        );
        assert!(message.contains(&expected), "{message}\nlacks: {expected}");
    }
}
