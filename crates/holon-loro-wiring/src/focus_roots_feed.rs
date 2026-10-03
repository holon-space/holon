//! Feeds the view engine the `focus_roots` matview: one stamp and one feed per
//! Sql commit that changes it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;

use holon_api::EntityUri;
use holon_api::Value;
use holon_api::commit_clock::CommitClock;
use holon_api::commit_clock::CommitSource;
use holon_api::commit_clock::Stamp;
use holon_core::storage::StorageEntity;
use holon_turso::commit_event::CommitChange;
use holon_turso::commit_event::CommitEvent;
use holon_turso::turso::DbHandle;
use holon_views::engine::ViewEngine;
use holon_views::error::EngineError;
use holon_views::views::FocusRoot;

const RELATION: &str = "focus_roots";
const SNAPSHOT_SQL: &str = "SELECT history_id, region, root_id FROM focus_roots";

/// A commit delivers its events one call at a time, so the state between them
/// lives here. Each call takes it out and puts the next one back; a call that
/// panics leaves it `Stopped`.
pub struct FocusRootsFeed {
    clock: Arc<CommitClock>,
    engine: Arc<ViewEngine>,
    listening: Mutex<Listening>,
    #[cfg(test)]
    panic_after_mint: std::sync::atomic::AtomicBool,
}

enum Listening {
    Between,
    /// `next` is the index of the commit's next event; `open` holds the
    /// commit's focus roots once one of its events changed them.
    In {
        commit_id: u64,
        next: u32,
        len: u32,
        open: Option<SqlCommit>,
    },
    Stopped,
}

/// The focus roots one Sql commit changed, under the stamp minted for it.
/// Dropped unfinished, it stops the engine with [`EngineError::TornCommit`].
struct SqlCommit {
    engine: Arc<ViewEngine>,
    stamp: Stamp,
    /// Per history id, each row image with its net count (+1 upserted, -1
    /// deleted), so the result does not depend on the order of the changes.
    rows: Option<BTreeMap<i64, Vec<(FocusRoot, i64)>>>,
}

impl FocusRootsFeed {
    /// Feeds `engine` the focus roots as of now, then every later commit's
    /// changes to them.
    pub async fn listen(
        db: &DbHandle,
        clock: Arc<CommitClock>,
        engine: Arc<ViewEngine>,
    ) -> anyhow::Result<Arc<FocusRootsFeed>> {
        let feed = Arc::new(FocusRootsFeed::new(clock, engine));
        let (on_event, on_snapshot) = (feed.clone(), feed.clone());
        db.listen_from_snapshot(
            SNAPSHOT_SQL,
            move |event| on_event.on_event(event),
            move |rows| on_snapshot.on_snapshot(rows),
        )
        .await
        .map_err(|e| anyhow::anyhow!("listen to {RELATION} from a snapshot: {e}"))?;
        Ok(feed)
    }

    fn new(clock: Arc<CommitClock>, engine: Arc<ViewEngine>) -> FocusRootsFeed {
        FocusRootsFeed {
            clock,
            engine,
            listening: Mutex::new(Listening::Between),
            #[cfg(test)]
            panic_after_mint: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn on_snapshot(&self, rows: Vec<StorageEntity>) {
        let stamp = self.clock.mint(CommitSource::Sql);
        match rows.iter().map(focus_root).collect() {
            Ok(roots) => self.engine.replace_focus_roots(stamp, roots),
            Err(reason) => {
                *self.lock() = Listening::Stopped;
                self.engine.stop(
                    CommitSource::Sql,
                    stamp,
                    EngineError::UnreadableRow {
                        store: CommitSource::Sql,
                        reason,
                    },
                );
            }
        }
    }

    fn on_event(&self, event: CommitEvent<'_>) {
        let listening = std::mem::replace(&mut *self.lock(), Listening::Stopped);
        let next = self.step(listening, &event);
        *self.lock() = next;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Listening> {
        self.listening
            .lock()
            .expect("no call panics while it holds the listening state")
    }

    fn step(&self, listening: Listening, event: &CommitEvent<'_>) -> Listening {
        let envelope = event.envelope();
        let mut open = match listening {
            Listening::Stopped => return Listening::Stopped,
            Listening::Between if envelope.index != 0 => {
                return self.refuse(None, format!("{envelope:?} starts no commit"));
            }
            Listening::Between => None,
            Listening::In {
                commit_id,
                next,
                len,
                open,
            } if (envelope.commit_id, envelope.index, envelope.len) != (commit_id, next, len) => {
                return self.refuse(
                    open,
                    format!("{envelope:?} arrived where event {next} of {len} of commit {commit_id} was due"),
                );
            }
            Listening::In { open, .. } => open,
        };
        if envelope.commit_id == 0 || envelope.index >= envelope.len {
            return self.refuse(open, format!("{envelope:?} is no position in a commit"));
        }
        if event.relation() == RELATION {
            let commit = open.get_or_insert_with(|| self.begin());
            if let Err(reason) = event.changes().and_then(|changes| commit.record(changes)) {
                open.take().expect("the commit was just opened").abort(
                    EngineError::UnreadableRow {
                        store: CommitSource::Sql,
                        reason,
                    },
                );
                return Listening::Stopped;
            }
        }
        if envelope.index + 1 < envelope.len {
            return Listening::In {
                commit_id: envelope.commit_id,
                next: envelope.index + 1,
                len: envelope.len,
                open,
            };
        }
        match open {
            Some(commit) => commit.finish(),
            None => Listening::Between,
        }
    }

    fn begin(&self) -> SqlCommit {
        let commit = SqlCommit {
            engine: self.engine.clone(),
            stamp: self.clock.mint(CommitSource::Sql),
            rows: Some(BTreeMap::new()),
        };
        #[cfg(test)]
        if self
            .panic_after_mint
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            panic!("injected panic after minting {:?}", commit.stamp);
        }
        commit
    }

    /// Stops the engine on an event outside its commit's envelope.
    fn refuse(&self, open: Option<SqlCommit>, reason: String) -> Listening {
        let error = EngineError::CommitEnvelope {
            store: CommitSource::Sql,
            reason: reason.clone(),
        };
        match open {
            Some(commit) => commit.abort(error),
            None => self.engine.stop(CommitSource::Sql, Stamp::NONE, error),
        }
        if cfg!(any(test, feature = "test-helpers")) {
            panic!("{RELATION} listener: {reason}");
        }
        Listening::Stopped
    }
}

impl SqlCommit {
    fn record(&mut self, changes: Vec<CommitChange>) -> Result<(), String> {
        let rows = self.rows.as_mut().expect("an open commit holds its rows");
        for change in changes {
            let (root, count) = match change {
                CommitChange::Upsert(row) => (focus_root(&row)?, 1),
                CommitChange::Delete(row) => (focus_root(&row)?, -1),
            };
            let images = rows.entry(root.history).or_default();
            match images.iter_mut().find(|(image, _)| *image == root) {
                Some((_, net)) => *net += count,
                None => images.push((root, count)),
            }
        }
        Ok(())
    }

    fn finish(mut self) -> Listening {
        let rows = self.rows.take().expect("an open commit holds its rows");
        let changes: Result<Vec<_>, String> = rows
            .into_iter()
            .filter_map(|(history, images)| net_change(history, images).transpose())
            .collect();
        match changes {
            Ok(changes) => {
                self.engine.feed_focus_roots(self.stamp, changes);
                Listening::Between
            }
            Err(reason) => {
                self.engine.stop(
                    CommitSource::Sql,
                    self.stamp,
                    EngineError::ConflictingChanges {
                        store: CommitSource::Sql,
                        reason,
                    },
                );
                Listening::Stopped
            }
        }
    }

    fn abort(mut self, error: EngineError) {
        self.rows = None;
        self.engine.stop(CommitSource::Sql, self.stamp, error);
    }
}

impl Drop for SqlCommit {
    fn drop(&mut self) {
        if self.rows.is_some() {
            self.engine.stop(
                CommitSource::Sql,
                self.stamp,
                EngineError::TornCommit {
                    store: CommitSource::Sql,
                    stamp: self.stamp,
                },
            );
        }
    }
}

/// The row `history` holds after the commit, or `None` when the commit leaves
/// it as it was. An image upserted and deleted in one commit cancels out.
fn net_change(
    history: i64,
    images: Vec<(FocusRoot, i64)>,
) -> Result<Option<(i64, Option<FocusRoot>)>, String> {
    let upserted: Vec<&FocusRoot> = images
        .iter()
        .filter(|(_, net)| *net == 1)
        .map(|(root, _)| root)
        .collect();
    let deleted = images.iter().filter(|(_, net)| *net == -1).count();
    let beyond_one = images.iter().any(|(_, net)| !(-1..=1).contains(net));
    match (upserted.as_slice(), deleted, beyond_one) {
        ([root], 0 | 1, false) => Ok(Some((history, Some((*root).clone())))),
        ([], 1, false) => Ok(Some((history, None))),
        ([], 0, false) => Ok(None),
        _ => Err(format!("history {history}: {images:?}")),
    }
}

pub fn focus_root(row: &StorageEntity) -> Result<FocusRoot, String> {
    let history = match row.get("history_id") {
        Some(Value::Integer(history)) => *history,
        other => return Err(format!("history_id is {other:?} in {row:?}")),
    };
    let region = match row.get("region") {
        Some(Value::String(region)) => region.clone(),
        other => return Err(format!("history {history}: region is {other:?}")),
    };
    let root = match row.get("root_id") {
        Some(Value::String(root)) => EntityUri::parse(root)
            .map_err(|e| format!("history {history}: root_id {root:?} is no URI: {e}"))?,
        other => return Err(format!("history {history}: root_id is {other:?}")),
    };
    if !root.is_block() {
        return Err(format!("history {history}: root {root} is no block"));
    }
    Ok(FocusRoot {
        history,
        region,
        root,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::panic::AssertUnwindSafe;
    use std::panic::catch_unwind;
    use std::sync::atomic::Ordering;

    use holon_api::ConditionBus;
    use holon_api::ConditionKind;
    use holon_api::condition_bus::VIEW_ENGINE_SUBJECT;
    use holon_turso::schema_module::SchemaModule;
    use holon_turso::schema_modules::NavigationSchemaModule;
    use holon_turso::turso::TursoBackend;
    use holon_views::engine::Field;
    use holon_views::engine::raise_on;
    use holon_views::views::View;
    use turso_core::DatabaseChange;
    use turso_core::DatabaseChangeType;
    use turso_core::types::ImmutableRecord;
    use turso_core::types::RelationChangeEvent;

    use super::*;

    struct World {
        _backend: TursoBackend,
        db: DbHandle,
        clock: Arc<CommitClock>,
        engine: Arc<ViewEngine>,
        conditions: Arc<ConditionBus>,
        feed: Arc<FocusRootsFeed>,
    }

    impl World {
        async fn new() -> World {
            let (backend, db) = TursoBackend::new_in_memory().await.unwrap();
            NavigationSchemaModule.ensure_schema(&db).await.unwrap();
            let (clock, conditions) = (Arc::new(CommitClock::new()), Arc::new(ConditionBus::new()));
            let engine = Arc::new(ViewEngine::start(
                clock.clone(),
                raise_on(conditions.clone()),
            ));
            let feed = FocusRootsFeed::listen(&db, clock.clone(), engine.clone())
                .await
                .unwrap();
            World {
                _backend: backend,
                db,
                clock,
                engine,
                conditions,
                feed,
            }
        }

        async fn execute(&self, sql: &str) {
            self.db
                .execute(sql, vec![])
                .await
                .unwrap_or_else(|e| panic!("{sql}: {e}"));
        }

        async fn navigate(&self, region: &str, root: &str) {
            self.execute(&format!(
                "INSERT INTO navigation_history (region, block_id) VALUES ('{region}', '{root}')"
            ))
            .await;
        }
    }

    fn stop_error(engine: &ViewEngine) -> EngineError {
        match engine.snapshot_and_subscribe(View::FocusRoots) {
            Err(error) => error,
            Ok((state, _)) => panic!("the engine runs, at {:?}", state.below),
        }
    }

    /// The stop reason the conditions name, one per raise.
    fn stops(conditions: &ConditionBus) -> Vec<String> {
        conditions
            .current()
            .into_iter()
            .map(|c| match c.reason {
                ConditionKind::ViewEngineStopped(reason) if c.subject == VIEW_ENGINE_SUBJECT => {
                    reason
                }
                other => panic!("{} raised {other:?}", c.subject),
            })
            .collect()
    }

    fn every_stamp_is_fed(clock: &CommitClock) {
        assert_eq!(
            clock.low_watermark().get(),
            clock.high_water().get() + 1,
            "outstanding Sql stamps: {:?}",
            clock.outstanding(CommitSource::Sql)
        );
    }

    fn event(
        relation: &str,
        commit_id: u64,
        commit_index: u32,
        commit_len: u32,
    ) -> RelationChangeEvent {
        RelationChangeEvent {
            relation_name: relation.to_string(),
            columns: vec![],
            changes: vec![],
            commit_id,
            commit_index,
            commit_len,
        }
    }

    enum Change {
        Insert,
        Delete,
    }

    /// A one-event commit of `changes` to `focus_roots` rows `(root, history)`
    /// in region `main`.
    fn focus_commit(commit_id: u64, changes: &[(Change, &str, i64)]) -> RelationChangeEvent {
        let changes = changes
            .iter()
            .map(|(change, root, history)| {
                let values = [
                    turso_core::Value::build_text("main"),
                    turso_core::Value::build_text(root.to_string()),
                    turso_core::Value::build_text("2026-10-03 00:00:00"),
                    turso_core::Value::from_i64(*history),
                ];
                let bin_record = ImmutableRecord::from_values(&values, values.len())
                    .unwrap()
                    .get_payload()
                    .to_vec();
                DatabaseChange {
                    change_id: 1,
                    change_time: 0,
                    change: match change {
                        Change::Insert => DatabaseChangeType::Insert { bin_record },
                        Change::Delete => DatabaseChangeType::Delete { bin_record },
                    },
                    table_name: RELATION.to_string(),
                    id: *history,
                }
            })
            .collect();
        RelationChangeEvent {
            columns: ["region", "root_id", "added_ts", "history_id"]
                .map(String::from)
                .to_vec(),
            changes,
            ..event(RELATION, commit_id, 0, 1)
        }
    }

    fn released(engine: &ViewEngine) -> BTreeSet<Vec<Field>> {
        let (state, _) = engine.snapshot_and_subscribe(View::FocusRoots).unwrap();
        state
            .deltas
            .into_iter()
            .map(|(row, n)| {
                assert_eq!(n, 1, "{row:?}");
                row
            })
            .collect()
    }

    fn main_root(root: &str, history: i64) -> Vec<Field> {
        vec![
            Field::Text("main".into()),
            Field::Uri(EntityUri::parse(root).unwrap()),
            Field::Int(history),
        ]
    }

    fn unstarted() -> (
        Arc<CommitClock>,
        Arc<ConditionBus>,
        Arc<ViewEngine>,
        FocusRootsFeed,
    ) {
        let clock = Arc::new(CommitClock::new());
        let conditions = Arc::new(ConditionBus::new());
        let engine = Arc::new(ViewEngine::start(
            clock.clone(),
            raise_on(conditions.clone()),
        ));
        let feed = FocusRootsFeed::new(clock.clone(), engine.clone());
        (clock, conditions, engine, feed)
    }

    /// The panic message of delivering `event`, which must panic.
    fn refused(feed: &FocusRootsFeed, event: &RelationChangeEvent) -> String {
        let panic = catch_unwind(AssertUnwindSafe(|| feed.on_event(CommitEvent::new(event))))
            .expect_err("an event outside its envelope panics in tests");
        panic
            .downcast_ref::<String>()
            .cloned()
            .expect("the refusal panics with its reason")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn each_commit_that_changes_focus_roots_is_one_released_version() {
        let w = World::new().await;
        let (start, versions) = w.engine.snapshot_and_subscribe(View::FocusRoots).unwrap();
        w.navigate("main", "block:a").await;
        w.navigate("right_sidebar", "block:b").await;
        w.db.transaction(vec![
            (
                "UPDATE navigation_history SET closed_at = datetime('now') \
                 WHERE region = 'main' AND closed_at IS NULL"
                    .to_string(),
                vec![],
            ),
            (
                "INSERT INTO navigation_history (region, block_id) VALUES ('main', 'block:c')"
                    .to_string(),
                vec![],
            ),
        ])
        .await
        .unwrap();
        w.execute("UPDATE navigation_history SET closed_at = datetime('now') WHERE region = 'right_sidebar'")
            .await;
        w.execute("INSERT INTO navigation_cursor (region, history_id) VALUES ('main', 3)")
            .await;

        every_stamp_is_fed(&w.clock);
        assert_eq!(w.clock.high_water().get(), start.below.get() - 1 + 4);
        let (state, _) = w.engine.snapshot_and_subscribe(View::FocusRoots).unwrap();
        let belows: Vec<u64> = versions.try_iter().map(|v| v.below.get()).collect();
        assert_eq!(
            belows,
            (start.below.get() + 1..=state.below.get()).collect::<Vec<_>>()
        );
        assert_eq!(belows.len(), 4);

        let released: BTreeSet<Vec<Field>> = state
            .deltas
            .into_iter()
            .map(|(row, n)| {
                assert_eq!(n, 1, "{row:?}");
                row
            })
            .collect();
        let authority: BTreeSet<Vec<Field>> =
            w.db.query(SNAPSHOT_SQL, Default::default())
                .await
                .unwrap()
                .iter()
                .map(|row| {
                    let root = focus_root(row).unwrap();
                    vec![
                        Field::Text(root.region.as_str().into()),
                        Field::Uri(root.root),
                        Field::Int(root.history),
                    ]
                })
                .collect();
        assert_eq!(
            released,
            BTreeSet::from([vec![
                Field::Text("main".into()),
                Field::Uri(EntityUri::parse("block:c").unwrap()),
                Field::Int(3),
            ]])
        );
        assert_eq!(released, authority);
        assert!(stops(&w.conditions).is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_commit_torn_after_its_mint_stops_the_engine_and_feeds_the_clock() {
        let w = World::new().await;
        w.feed.panic_after_mint.store(true, Ordering::SeqCst);
        w.navigate("main", "block:a").await;
        let torn = w.clock.high_water();

        let error = EngineError::TornCommit {
            store: CommitSource::Sql,
            stamp: torn,
        };
        assert_eq!(stop_error(&w.engine), error);
        assert_eq!(stops(&w.conditions), vec![error.to_string()]);
        every_stamp_is_fed(&w.clock);

        w.navigate("main", "block:b").await;
        assert_eq!(
            w.clock.high_water(),
            torn,
            "a stopped listener mints nothing"
        );
    }

    #[test]
    fn a_commit_id_of_zero_stops_the_engine() {
        let clock = Arc::new(CommitClock::new());
        let conditions = Arc::new(ConditionBus::new());
        let engine = Arc::new(ViewEngine::start(
            clock.clone(),
            raise_on(conditions.clone()),
        ));
        let feed = FocusRootsFeed::new(clock.clone(), engine.clone());

        let reason = refused(&feed, &event("blocks", 0, 0, 1));
        assert!(reason.contains("is no position in a commit"), "{reason}");
        let error = stop_error(&engine);
        assert!(
            matches!(
                &error,
                EngineError::CommitEnvelope {
                    store: CommitSource::Sql,
                    ..
                }
            ),
            "{error:?}"
        );
        assert_eq!(stops(&conditions), vec![error.to_string()]);
        every_stamp_is_fed(&clock);
    }

    #[test]
    fn an_event_out_of_its_commit_order_stops_the_engine_through_the_open_stamp() {
        let clock = Arc::new(CommitClock::new());
        let conditions = Arc::new(ConditionBus::new());
        let engine = Arc::new(ViewEngine::start(
            clock.clone(),
            raise_on(conditions.clone()),
        ));
        let feed = FocusRootsFeed::new(clock.clone(), engine.clone());

        feed.on_event(CommitEvent::new(&event(RELATION, 7, 0, 3)));
        let open = clock.high_water();
        assert_eq!(clock.outstanding(CommitSource::Sql), vec![open]);
        let reason = refused(&feed, &event("blocks", 7, 2, 3));
        assert!(
            reason.contains("where event 1 of 3 of commit 7 was due"),
            "{reason}"
        );

        let error = stop_error(&engine);
        assert!(
            matches!(
                &error,
                EngineError::CommitEnvelope {
                    store: CommitSource::Sql,
                    ..
                }
            ),
            "{error:?}"
        );
        assert_eq!(stops(&conditions), vec![error.to_string()]);
        every_stamp_is_fed(&clock);

        feed.on_event(CommitEvent::new(&event(RELATION, 8, 0, 1)));
        assert_eq!(clock.high_water(), open, "a stopped listener mints nothing");
    }

    #[test]
    fn a_commit_nets_its_changes_in_any_order() {
        let (clock, conditions, engine, feed) = unstarted();
        let deliver = |event: RelationChangeEvent| feed.on_event(CommitEvent::new(&event));
        deliver(focus_commit(
            1,
            &[
                (Change::Insert, "block:a", 1),
                (Change::Insert, "block:x", 2),
            ],
        ));

        deliver(focus_commit(
            2,
            &[
                (Change::Insert, "block:b", 1),
                (Change::Delete, "block:a", 1),
                (Change::Insert, "block:y", 3),
                (Change::Delete, "block:y", 3),
            ],
        ));
        assert_eq!(
            released(&engine),
            BTreeSet::from([main_root("block:b", 1), main_root("block:x", 2)])
        );

        deliver(focus_commit(
            3,
            &[
                (Change::Delete, "block:b", 1),
                (Change::Insert, "block:c", 1),
            ],
        ));
        assert_eq!(
            released(&engine),
            BTreeSet::from([main_root("block:c", 1), main_root("block:x", 2)])
        );
        assert!(stops(&conditions).is_empty());
        every_stamp_is_fed(&clock);
    }

    #[test]
    fn changes_that_net_to_two_rows_for_one_history_stop_the_engine() {
        let (clock, conditions, engine, feed) = unstarted();
        feed.on_event(CommitEvent::new(&focus_commit(
            1,
            &[
                (Change::Insert, "block:a", 1),
                (Change::Insert, "block:b", 1),
            ],
        )));

        let error = stop_error(&engine);
        assert!(
            matches!(
                &error,
                EngineError::ConflictingChanges {
                    store: CommitSource::Sql,
                    reason,
                } if reason.starts_with("history 1:")
            ),
            "{error:?}"
        );
        assert_eq!(stops(&conditions), vec![error.to_string()]);
        every_stamp_is_fed(&clock);
    }
}
