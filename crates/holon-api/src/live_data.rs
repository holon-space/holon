//! CDC-aware self-updating collection keyed by entity ID.
//!
//! Uses `futures_signals::signal_map::MutableBTreeMap` for reactive storage:
//! change events are broadcast natively as `MapDiff` values — no separate
//! version channel needed.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use anyhow::Result;
use futures_signals::signal_map::MutableBTreeMap;
use futures_signals::signal_map::MutableBTreeMapLockRef;
use futures_signals::signal_map::MutableSignalMap;
use tokio::sync::Notify;

use crate::Change;
use crate::StorageEntity;
use crate::Value;

pub mod group_by;
pub mod home_by;
pub mod supervision;

/// Pull `_rowid` (set by `process_cdc_event`) out of a row's data, if present.
fn extract_rowid(data: &StorageEntity) -> Option<String> {
    match data.get("_rowid")? {
        Value::String(s) => Some(s.clone()),
        Value::Integer(i) => Some(i.to_string()),
        _ => None,
    }
}

/// A live, CDC-driven collection of items keyed by entity ID.
///
/// `T` is the item type. Items are parsed from `StorageEntity` (HashMap<String,
/// Value>) via the `parse_fn` provided at construction.
/// CDC events (Created/Updated/Deleted) are applied incrementally.
///
/// Change notifications are emitted reactively via `MutableBTreeMap`'s signal
/// map. Consumers can subscribe via [`signal_map()`](Self::signal_map) for
/// push-based cache invalidation.
///
/// Both `id_fn` and `parse_fn` return `Result` — if they fail, it's a
/// programming error (wrong table, schema mismatch) and should be loud, not
/// silently swallowed.
type IdFn = Box<dyn Fn(&StorageEntity) -> Result<String> + Send + Sync>;
type ParseFn<T> = Box<dyn Fn(&StorageEntity) -> Result<T> + Send + Sync>;

pub struct LiveData<T: Clone + Send + Sync + 'static> {
    items: MutableBTreeMap<String, Arc<T>>,
    id_fn: IdFn,
    parse_fn: ParseFn<T>,
    /// Highest `BatchMetadata.seq` this mirror has applied via `subscribe`'s
    /// CDC stream. Tests use this with `cdc_emitted_watermark()` to wait for
    /// the mirror to catch up — see `wait_for_seq`. `0` until the first
    /// CDC batch is applied (initial-load rows don't carry a seq).
    last_consumed_seq: Arc<AtomicU64>,
    /// Signalled after every `last_consumed_seq` advance so `wait_for_seq`
    /// can wake immediately instead of polling on a 1ms timer.
    seq_advanced: Arc<Notify>,
    /// Signalled after *any* mutation of `items` — CDC `apply_changes`,
    /// the `subscribe` actor, and the optimistic `insert` bypass alike.
    /// `wait_until` waits on this so it wakes regardless of how the awaited
    /// rows arrived (unlike `seq_advanced`, which fires only for seq-tagged
    /// CDC batches).
    items_changed: Arc<Notify>,
    /// Maps Turso's internal `rowid` (strings, populated from `_rowid` in
    /// CDC `data`) to the user-defined key produced by `id_fn`.
    ///
    /// Why: matview rows without an `id` column (e.g. `focus_roots`,
    /// `block_tags`) make `process_cdc_event` fall back to `change.id`
    /// (the rowid) for `Change::Deleted.id`, while `Created/Updated`
    /// events flow through the user's `id_fn` which usually computes a
    /// composite key from the row data. Without this map the delete
    /// would silently miss the existing entry, leaving the LiveData
    /// stale relative to its underlying matview — exactly the
    /// `right_sidebar` `focus_roots` flake after `NavigateBack`.
    rowid_to_key: Mutex<HashMap<String, String>>,
}

impl<T: Clone + Send + Sync + 'static> LiveData<T> {
    pub fn new(
        initial_rows: Vec<StorageEntity>,
        id_fn: impl Fn(&StorageEntity) -> Result<String> + Send + Sync + 'static,
        parse_fn: impl Fn(&StorageEntity) -> Result<T> + Send + Sync + 'static,
    ) -> Arc<Self> {
        let mut items = BTreeMap::new();
        let mut rowid_to_key: HashMap<String, String> = HashMap::new();
        for row in initial_rows {
            let id = (id_fn)(&row).expect("id_fn failed on initial row");
            let parsed = (parse_fn)(&row).expect("parse_fn failed on initial row");
            // Initial rows from `MatviewManager::query_view` carry `_rowid`
            // when it's available; record so subsequent CDC `Delete` events
            // (keyed by rowid for matviews without an `id` column) can find
            // the right entry to remove. Rows without `_rowid` (e.g. tables
            // where the entity id matches the user key directly) just skip
            // — the direct id removal in `apply_changes` handles them.
            if let Some(rowid) = extract_rowid(&row) {
                rowid_to_key.insert(rowid, id.clone());
            }
            items.insert(id, Arc::new(parsed));
        }

        Arc::new(Self {
            items: MutableBTreeMap::with_values(items),
            id_fn: Box::new(id_fn),
            parse_fn: Box::new(parse_fn),
            last_consumed_seq: Arc::new(AtomicU64::new(0)),
            seq_advanced: Arc::new(Notify::new()),
            items_changed: Arc::new(Notify::new()),
            rowid_to_key: Mutex::new(rowid_to_key),
        })
    }

    /// Highest CDC `seq` this mirror has applied. `0` means "no CDC batch
    /// applied yet" (initial-load rows are not seq-tagged).
    pub fn consumed_seq(&self) -> u64 {
        self.last_consumed_seq.load(Ordering::SeqCst)
    }

    /// Wait until this mirror is *quiescent* — no new CDC batch has been
    /// applied for `quiet_for`. Bounded by an overall `timeout`.
    ///
    /// Why not `wait_for_seq(target)` against `cdc_emitted_watermark()`? That
    /// API is fundamentally mismatched: `cdc_emitted_watermark` is a global
    /// counter incremented by *every* matview's batches, but each mirror only
    /// sees its own matview's batches (filtered by the `MatviewManager` demux).
    /// So a mirror's `consumed_seq` only reaches the global watermark by
    /// coincidence — every wait timed out (verified by trace 2026-05-19).
    ///
    /// Quiescence is what callers actually need: "the mirror has drained
    /// everything pending for its source." `apply_changes` calls
    /// `notify_waiters` after every successful batch, so we reset the
    /// quiet-deadline each time the notify fires and only return when the
    /// silence has lasted `quiet_for`.
    ///
    /// Returns silently if the overall `timeout` fires — callers treat that
    /// as a probe failure, not a hard fault.
    pub async fn wait_for_quiescent(
        &self,
        quiet_for: std::time::Duration,
        timeout: std::time::Duration,
    ) {
        use tracing::field;
        let span = tracing::info_span!(
            "live_data.wait_for_quiescent",
            quiet_for_ms = quiet_for.as_millis() as u64,
            timeout_ms = timeout.as_millis() as u64,
            batches_seen = field::Empty,
            final_seq = field::Empty,
            timed_out = field::Empty,
        );
        let _enter = span.enter();
        let overall = tokio::time::sleep(timeout);
        tokio::pin!(overall);
        let mut batches_seen: u64 = 0;
        loop {
            // Arm the notification BEFORE the seq snapshot so a notify
            // landing between snapshot and await still wakes us.
            let notified = self.seq_advanced.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let quiet = tokio::time::sleep(quiet_for);
            tokio::pin!(quiet);
            tokio::select! {
                _ = &mut notified => {
                    batches_seen += 1;
                    // Loop and re-arm; another batch may be inbound.
                }
                _ = &mut quiet => {
                    span.record("batches_seen", batches_seen);
                    span.record("final_seq", self.consumed_seq());
                    span.record("timed_out", false);
                    return;
                }
                _ = &mut overall => {
                    span.record("batches_seen", batches_seen);
                    span.record("final_seq", self.consumed_seq());
                    span.record("timed_out", true);
                    return;
                }
            }
        }
    }

    /// Wait until `predicate` holds over the current item snapshot, or until
    /// `timeout` elapses. Returns `true` if the predicate was satisfied,
    /// `false` on timeout.
    ///
    /// This is the **positional** catch-up primitive: unlike
    /// [`wait_for_quiescent`](Self::wait_for_quiescent), which returns once the
    /// CDC stream has merely gone *quiet*, `wait_until` returns only once the
    /// caller's condition is observed in the data. That distinction is
    /// load-bearing for the block-sync cache wait it replaces
    /// (`wait_for_cache_caught_up`): a quiescence-only wait can return *before*
    /// the projection that produces the awaited rows has even begun (a
    /// momentary lull), reintroducing the stale-cache re-render race. A
    /// predicate over block presence/count cannot return early.
    ///
    /// The notification is armed BEFORE the snapshot so a CDC batch landing
    /// between the predicate check and the await still wakes us (no lost
    /// wakeup), mirroring `wait_for_quiescent`.
    pub async fn wait_until<F>(&self, mut predicate: F, timeout: std::time::Duration) -> bool
    where
        F: FnMut(&BTreeMap<String, Arc<T>>) -> bool,
    {
        let overall = tokio::time::sleep(timeout);
        tokio::pin!(overall);
        loop {
            // Arm the notification BEFORE reading so a mutation applied between
            // the read and the await is not missed.
            let notified = self.items_changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            if predicate(&self.items.lock_ref()) {
                return true;
            }

            tokio::select! {
                _ = &mut notified => {
                    // A new batch landed; re-check the predicate.
                }
                _ = &mut overall => {
                    // Final check in case the predicate became true exactly as
                    // the deadline fired.
                    return predicate(&self.items.lock_ref());
                }
            }
        }
    }

    /// Read the current snapshot. Returns a guard — hold briefly.
    ///
    /// The guard `Deref`s to `BTreeMap<String, Arc<T>>`, so callers can use
    /// `.get(&key)`, `.values()`, `.len()`, etc. directly.
    pub fn read(&self) -> MutableBTreeMapLockRef<'_, String, Arc<T>> {
        self.items.lock_ref()
    }

    /// Get a reactive signal map that emits `MapDiff` on every change.
    ///
    /// The signal emits an initial `MapDiff::Replace` with all current entries,
    /// followed by `MapDiff::Insert` / `MapDiff::Update` / `MapDiff::Remove`
    /// for each subsequent change.
    ///
    /// Use [`SignalMapExt::for_each`] (or [`SignalMapExt::key_cloned`]) to
    /// react to changes in a background task.
    pub fn signal_map(&self) -> MutableSignalMap<String, Arc<T>> {
        self.items.signal_map_cloned()
    }

    /// Insert or update an item directly (bypasses CDC).
    ///
    /// Use this for optimistic cache updates after a write to ensure
    /// the LiveData is immediately consistent, without waiting for the
    /// CDC roundtrip through matview → stream → apply_changes.
    pub fn insert(&self, key: String, value: Arc<T>) {
        self.items.lock_mut().insert_cloned(key, value);
        self.items_changed.notify_waiters();
    }

    /// Apply a batch of CDC changes incrementally.
    pub fn apply_changes(&self, changes: Vec<Change<StorageEntity>>) {
        let applied_any = !changes.is_empty();
        let mut lock = self.items.lock_mut();
        let mut rowid_map = self
            .rowid_to_key
            .lock()
            .expect("rowid_to_key mutex poisoned");
        for change in changes {
            match change {
                Change::Created { data, .. } | Change::Updated { data, .. } => {
                    // A single un-keyable / un-parseable CDC row must NOT kill the
                    // whole live feed. This runs on the detached `subscribe` actor
                    // task, so an `.expect()` panic here unwinds ONLY that task —
                    // tokio swallows it, the CDC stream is never drained again, and
                    // every downstream mirror (org write-back, link indexer, the
                    // ViewModel) silently freezes with no banner (BugFunnel row 24:
                    // "feed DIES silently mid-session, app looks healthy"). That is
                    // the "silently degrades to look fine" anti-pattern this repo
                    // forbids. Fail LOUD (ERROR — captured by `inv-no-observed-errors`
                    // and visible in prod logs) and DROP just this row, keeping the
                    // feed alive for every subsequent good row (disclosed degradation).
                    let id = match (self.id_fn)(&data) {
                        Ok(id) => id,
                        Err(e) => {
                            tracing::error!(
                                "LiveData: id_fn failed on a CDC row — DROPPING this row \
                                 and keeping the feed alive (degraded): {e:#}; row={data:?}"
                            );
                            continue;
                        }
                    };
                    let parsed = match (self.parse_fn)(&data) {
                        Ok(parsed) => parsed,
                        Err(e) => {
                            tracing::error!(
                                "LiveData: parse_fn failed on CDC row id={id} — DROPPING \
                                 this row and keeping the feed alive (degraded): {e:#}"
                            );
                            continue;
                        }
                    };
                    if let Some(rowid) = extract_rowid(&data) {
                        rowid_map.insert(rowid, id.clone());
                    }
                    lock.insert_cloned(id, Arc::new(parsed));
                }
                Change::Deleted { id, .. } => {
                    // For matviews without an `id` column, `process_cdc_event`
                    // sets `Change::Deleted.id` to the rowid string —
                    // `id_fn`'s composite key never matches that. Try direct
                    // removal first (the entity-id case), then fall back to
                    // the rowid → user-key map populated by Created/Updated.
                    if lock.remove(&id).is_some() {
                        // Direct match — drop any rowid mapping that pointed
                        // at this user key so the map doesn't grow stale.
                        rowid_map.retain(|_, v| v != &id);
                    } else if let Some(user_key) = rowid_map.remove(&id) {
                        lock.remove(&user_key);
                    }
                }
                Change::FieldsChanged { entity_id, .. } => {
                    // FieldsChanged is never produced for LiveData-backed matviews;
                    // its arrival signals an upstream defect. Surface it LOUDLY, but
                    // do not `panic!` — this actor is detached, so a panic here would
                    // silently kill the whole feed (see the Created/Updated arm above,
                    // BugFunnel row 24) instead of just flagging the bad change.
                    tracing::error!(
                        "LiveData: unexpected FieldsChanged for {entity_id} — ignoring \
                         this change and keeping the feed alive (upstream defect)"
                    );
                }
            }
        }
        drop(rowid_map);
        drop(lock);
        if applied_any {
            self.items_changed.notify_waiters();
        }
    }

    /// Spawn a background task that listens to the CDC stream and applies
    /// changes.
    ///
    /// Each batch's `metadata.seq` is recorded after `apply_changes` returns,
    /// so callers can `wait_for_seq` until the mirror has caught up to a
    /// specific CDC watermark.
    ///
    /// `source_name` should identify the underlying table/matview (e.g.
    /// "block", "focus_roots") so traces can tell mirrors apart when one
    /// stops draining.
    pub fn subscribe<C, S>(self: &Arc<Self>, source_name: &'static str, mut stream: S)
    where
        C: Into<Change<StorageEntity>> + Send + 'static,
        S: tokio_stream::Stream<Item = crate::streaming::BatchWithMetadata<C>>
            + Send
            + Unpin
            + 'static,
    {
        let live = Arc::clone(self);
        tokio::spawn(async move {
            use tokio_stream::StreamExt;
            use tracing::Instrument;
            let span = tracing::info_span!("live_data.subscribe_actor", source = source_name);
            let _enter = span.enter();
            let mut batches_seen: u64 = 0;
            loop {
                let next = async { stream.next().await }
                    .instrument(tracing::info_span!(
                        "live_data.stream_next",
                        source = source_name,
                    ))
                    .await;
                let Some(batch) = next else {
                    tracing::warn!(
                        source = source_name,
                        batches_seen,
                        last_seq = live.last_consumed_seq.load(Ordering::SeqCst),
                        "LiveData stream ended — actor exiting"
                    );
                    return;
                };
                batches_seen += 1;
                let seq = batch.metadata.seq;
                let changes: Vec<Change<StorageEntity>> =
                    batch.inner.items.into_iter().map(Into::into).collect();
                let change_count = changes.len();
                // Entities this batch makes visible — feeds the e2e
                // interaction-latency correlator AFTER apply below, as
                // `(id, WriteSeq?)` pairs. The row's own `id` carries its
                // `write_seq` op-instance token (editor content writes); the
                // `parent_id` correlation (a create/split op dispatched at a
                // parent completes via its new child's row) is always tokenless.
                let touched: Vec<(String, Option<crate::write_seq::WriteSeq>)> = changes
                    .iter()
                    .flat_map(|c| {
                        let row_pairs = |data: &StorageEntity| {
                            let seq = data
                                .get("write_seq")
                                .and_then(|v| v.as_i64())
                                .filter(|&s| s > 0)
                                .map(crate::write_seq::WriteSeq::from_i64);
                            let mut out = Vec::new();
                            if let Some(id) = data.get("id").and_then(|v| v.as_string()) {
                                out.push((id.to_string(), seq));
                            }
                            if let Some(pid) = data.get("parent_id").and_then(|v| v.as_string()) {
                                out.push((pid.to_string(), None));
                            }
                            out
                        };
                        match c {
                            Change::Created { data, .. } => row_pairs(data),
                            Change::Updated { id, data, .. } => {
                                let mut pairs = row_pairs(data);
                                if !pairs.iter().any(|(pid, _)| pid == id) {
                                    pairs.push((id.clone(), None));
                                }
                                pairs
                            }
                            Change::Deleted { id, .. } => vec![(id.clone(), None)],
                            Change::FieldsChanged { entity_id, .. } => {
                                vec![(entity_id.clone(), None)]
                            }
                        }
                    })
                    .collect();
                let t_rows = std::time::Instant::now();
                live.apply_changes(changes);
                crate::latency_e2e::rows_delivered(
                    source_name,
                    touched.iter().map(|(id, seq)| (id.as_str(), *seq)),
                );
                if seq > 0 {
                    live.last_consumed_seq.store(seq, Ordering::SeqCst);
                    live.seq_advanced.notify_waiters();
                }
                tracing::trace!(
                    source = source_name,
                    seq,
                    changes = change_count,
                    "LiveData batch applied"
                );
                // Latency stage (projection->rows): a CDC batch from the matview
                // lands and the reactive mirror applies it — the point at which a
                // projected change becomes visible to the view model / widgets
                // (final GPU paint excluded). Greppable via target="holon_latency".
                if change_count > 0 {
                    tracing::info!(
                        target: "holon_latency",
                        stage = "rows",
                        source = source_name,
                        rows = change_count,
                        seq,
                        ms = t_rows.elapsed().as_millis() as u64,
                        "holon_latency",
                    );
                }
            }
        });
    }
}

/// The shared convergent block feed (`LiveData<Block>` over the block
/// matview's CDC stream at the composition root). Built once and shared by
/// every downstream sink that needs it (the Loro→SQL controller keeps it
/// alive; the link indexer drives `block_link` off it; the org controller's
/// re-render resolver reads it). A newtype so DI hands back the inner `Arc`
/// cleanly (`LiveData::new` already returns an `Arc`). Available in both
/// modes — the backing matview exists with or without Loro.
pub struct BlockFeed(pub Arc<LiveData<crate::block::Block>>);

#[cfg(test)]
mod tests {
    use std::task::Context;
    use std::task::Poll;

    use futures_signals::signal_map::MapDiff;
    use futures_signals::signal_map::SignalMapExt;

    use super::*;
    use crate::Value;

    fn make_row(id: &str, content: &str) -> StorageEntity {
        let mut row: StorageEntity = crate::StorageEntity::new();
        row.insert("id".into(), Value::String(id.to_string()));
        row.insert("content".into(), Value::String(content.to_string()));
        row
    }

    #[test]
    fn test_live_data_initial_load() {
        let rows = vec![make_row("a", "hello"), make_row("b", "world")];
        let live: Arc<LiveData<String>> = LiveData::new(
            rows,
            |row| Ok(row.get("id").unwrap().as_string().unwrap().to_string()),
            |row| Ok(row.get("content").unwrap().as_string().unwrap().to_string()),
        );

        let items = live.read();
        assert_eq!(items.len(), 2);
        assert_eq!(**items.get("a").unwrap(), "hello");
        assert_eq!(**items.get("b").unwrap(), "world");
    }

    #[test]
    fn test_apply_changes() {
        let live: Arc<LiveData<String>> = LiveData::new(
            vec![],
            |row| Ok(row.get("id").unwrap().as_string().unwrap().to_string()),
            |row| Ok(row.get("content").unwrap().as_string().unwrap().to_string()),
        );

        let created = Change::Created {
            data: make_row("c", "new"),
            origin: crate::ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        };
        live.apply_changes(vec![created]);

        let items = live.read();
        assert_eq!(**items.get("c").unwrap(), "new");
    }

    /// BugFunnel row 24 (apply-level): a single un-parseable CDC row must NOT
    /// abort the batch — the good row in the same batch still lands, and the
    /// row dropped is disclosed via an ERROR log. Before the fix
    /// `apply_changes` `.expect()`-panicked on the bad row, which (on the
    /// detached `subscribe` actor) silently killed the whole live feed.
    #[test]
    fn apply_changes_drops_unparseable_row_keeps_feed_alive() {
        let live: Arc<LiveData<String>> = LiveData::new(
            vec![],
            |row| Ok(row.get("id").unwrap().as_string().unwrap().to_string()),
            |row| {
                let content = row.get("content").unwrap().as_string().unwrap();
                if content == "POISON" {
                    anyhow::bail!("poison row rejected by parse_fn");
                }
                Ok(content.to_string())
            },
        );

        let local = || crate::ChangeOrigin::Local {
            operation_id: None,
            trace_id: None,
        };
        live.apply_changes(vec![
            Change::Created {
                data: make_row("bad", "POISON"),
                origin: local(),
            },
            Change::Created {
                data: make_row("good", "fine"),
                origin: local(),
            },
        ]);

        let items = live.read();
        assert!(
            items.get("bad").is_none(),
            "the un-parseable row must be dropped, not inserted"
        );
        assert_eq!(
            **items.get("good").unwrap(),
            "fine",
            "the good row in the same batch must still land (feed stays alive)"
        );
    }

    /// BugFunnel row 24 (actor-level): the `subscribe` background actor must
    /// survive a poison row and keep draining the stream. Before the fix the
    /// `apply_changes` panic unwound the actor task (swallowed by tokio), the
    /// CDC stream was never drained again, and every downstream mirror froze
    /// silently — "feed DIES silently mid-session, app looks healthy".
    #[tokio::test]
    async fn subscribe_actor_survives_unparseable_row() {
        let live: Arc<LiveData<String>> = LiveData::new(
            vec![],
            |row| Ok(row.get("id").unwrap().as_string().unwrap().to_string()),
            |row| {
                let content = row.get("content").unwrap().as_string().unwrap();
                if content == "POISON" {
                    anyhow::bail!("poison row rejected by parse_fn");
                }
                Ok(content.to_string())
            },
        );

        let local = || crate::ChangeOrigin::Local {
            operation_id: None,
            trace_id: None,
        };
        let batch = |seq: u64, change: Change<StorageEntity>| crate::streaming::WithMetadata {
            inner: crate::streaming::Batch {
                items: vec![change],
            },
            metadata: crate::streaming::BatchMetadata {
                seq,
                ..Default::default()
            },
        };

        // Poison batch FIRST (would have killed the pre-fix actor), then a good
        // batch that must still be delivered.
        let stream = tokio_stream::iter(vec![
            batch(
                1,
                Change::Created {
                    data: make_row("bad", "POISON"),
                    origin: local(),
                },
            ),
            batch(
                2,
                Change::Created {
                    data: make_row("good", "fine"),
                    origin: local(),
                },
            ),
        ]);

        live.subscribe("test_feed", stream);

        let landed = live
            .wait_until(
                |m| m.contains_key("good"),
                std::time::Duration::from_secs(5),
            )
            .await;
        assert!(
            landed,
            "the actor must survive the poison row and deliver the subsequent good row"
        );
        assert!(
            live.read().get("bad").is_none(),
            "the poison row must have been dropped"
        );
    }

    #[test]
    fn test_apply_delete() {
        let live: Arc<LiveData<String>> = LiveData::new(
            vec![make_row("x", "data")],
            |row| Ok(row.get("id").unwrap().as_string().unwrap().to_string()),
            |row| Ok(row.get("content").unwrap().as_string().unwrap().to_string()),
        );

        assert_eq!(live.read().len(), 1);

        let deleted = Change::Deleted {
            id: "x".to_string(),
            origin: crate::ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        };
        live.apply_changes(vec![deleted]);

        assert_eq!(live.read().len(), 0);
    }

    /// Reproduces the focus_roots stale-Vec bug: matview rows with no `id`
    /// column are deleted via `Change::Deleted { id: rowid, .. }`, but the
    /// LiveData stores items keyed by id_fn output (e.g. composite
    /// "region|root_id"). Without rowid → user_key tracking, the delete
    /// silently misses and the stale entry lingers.
    #[test]
    fn test_apply_delete_composite_key_via_rowid() {
        // id_fn: composite key from (region, root_id) — mirrors `live_focus_roots`.
        let live: Arc<LiveData<String>> = LiveData::new(
            vec![],
            |row| {
                let region = row.get("region").unwrap().as_string().unwrap();
                let root_id = row.get("root_id").unwrap().as_string().unwrap();
                Ok(format!("{region}\u{1F}{root_id}"))
            },
            |row| Ok(row.get("root_id").unwrap().as_string().unwrap().to_string()),
        );

        // Simulate a CDC Created event the matview emits for a new focus_roots
        // row. process_cdc_event injects `_rowid` into the data — this is what
        // our fix uses to remember the rowid → user-key mapping.
        let mut row: StorageEntity = crate::StorageEntity::new();
        row.insert("region".into(), Value::String("right_sidebar".to_string()));
        row.insert("root_id".into(), Value::String("block:abc".to_string()));
        row.insert("_rowid".into(), Value::String("42".to_string()));

        live.apply_changes(vec![Change::Created {
            data: row,
            origin: crate::ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        }]);

        assert_eq!(live.read().len(), 1, "Created should have inserted 1 row");

        // CDC Delete for the same matview row. `process_cdc_event` falls back
        // to `change.id.to_string()` (rowid) because data has no `id` column.
        live.apply_changes(vec![Change::Deleted {
            id: "42".to_string(),
            origin: crate::ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        }]);

        assert_eq!(
            live.read().len(),
            0,
            "rowid-keyed delete must remove the composite-key entry"
        );
    }

    #[test]
    fn test_signal_map_notifies() {
        let live: Arc<LiveData<String>> = LiveData::new(
            vec![make_row("pre", "seeded")],
            |row| Ok(row.get("id").unwrap().as_string().unwrap().to_string()),
            |row| Ok(row.get("content").unwrap().as_string().unwrap().to_string()),
        );

        let mut signal = live.signal_map();
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        // First poll: initial MapDiff::Replace with pre-seeded entries
        match signal.poll_map_change_unpin(&mut cx) {
            Poll::Ready(Some(MapDiff::Replace { entries })) => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].0, "pre");
                assert_eq!(*entries[0].1, "seeded");
            }
            other => panic!("Expected Ready(Some(Replace)), got {:?}", other),
        }

        // Apply a CDC change
        live.apply_changes(vec![Change::Created {
            data: make_row("a", "new-item"),
            origin: crate::ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        }]);

        // Second poll: MapDiff::Insert for the new entry
        match signal.poll_map_change_unpin(&mut cx) {
            Poll::Ready(Some(MapDiff::Insert { key, value })) => {
                assert_eq!(key, "a");
                assert_eq!(*value, "new-item");
            }
            other => panic!("Expected Ready(Some(Insert)), got {:?}", other),
        }
    }
}
