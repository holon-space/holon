//! CDC-aware self-updating collection keyed by entity ID.
//!
//! Uses `futures_signals::signal_map::MutableBTreeMap` for reactive storage:
//! change events are broadcast natively as `MapDiff` values — no separate
//! version channel needed.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use futures_signals::signal_map::{MutableBTreeMap, MutableBTreeMapLockRef, MutableSignalMap};
use holon_api::{Change, Value};

use crate::storage::turso::RowChange;
use crate::storage::types::StorageEntity;

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
/// `T` is the item type. Items are parsed from `StorageEntity` (HashMap<String, Value>)
/// via the `parse_fn` provided at construction.
/// CDC events (Created/Updated/Deleted) are applied incrementally.
///
/// Change notifications are emitted reactively via `MutableBTreeMap`'s signal map.
/// Consumers can subscribe via [`signal_map()`](Self::signal_map) for push-based
/// cache invalidation.
///
/// Both `id_fn` and `parse_fn` return `Result` — if they fail, it's a programming
/// error (wrong table, schema mismatch) and should be loud, not silently swallowed.
pub struct LiveData<T: Clone + Send + Sync + 'static> {
    items: MutableBTreeMap<String, T>,
    id_fn: Box<dyn Fn(&StorageEntity) -> Result<String> + Send + Sync>,
    parse_fn: Box<dyn Fn(&StorageEntity) -> Result<T> + Send + Sync>,
    /// Highest `BatchMetadata.seq` this mirror has applied via `subscribe`'s
    /// CDC stream. Tests use this with `cdc_emitted_watermark()` to wait for
    /// the mirror to catch up — see `wait_for_seq`. `0` until the first
    /// CDC batch is applied (initial-load rows don't carry a seq).
    last_consumed_seq: Arc<AtomicU64>,
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
            items.insert(id, parsed);
        }

        Arc::new(Self {
            items: MutableBTreeMap::with_values(items),
            id_fn: Box::new(id_fn),
            parse_fn: Box::new(parse_fn),
            last_consumed_seq: Arc::new(AtomicU64::new(0)),
            rowid_to_key: Mutex::new(rowid_to_key),
        })
    }

    /// Highest CDC `seq` this mirror has applied. `0` means "no CDC batch
    /// applied yet" (initial-load rows are not seq-tagged).
    pub fn consumed_seq(&self) -> u64 {
        self.last_consumed_seq.load(Ordering::SeqCst)
    }

    /// Wait until this mirror has applied every CDC batch up to and including
    /// `target_seq`. Pair with `TursoBackend::cdc_emitted_watermark()` sampled
    /// before the wait — `subscribe` updates the consumed seq after each
    /// `apply_changes`, so once `consumed_seq() >= target_seq` the mirror is
    /// guaranteed to reflect every batch the matview emitted before sampling.
    ///
    /// Returns silently on timeout — callers should treat that as a probe
    /// failure (likely unrelated wedge upstream) rather than a hard fault.
    pub async fn wait_for_seq(&self, target_seq: u64, timeout: std::time::Duration) {
        if target_seq == 0 {
            return;
        }
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.consumed_seq() >= target_seq {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    }

    /// Read the current snapshot. Returns a guard — hold briefly.
    ///
    /// The guard `Deref`s to `BTreeMap<String, T>`, so callers can use
    /// `.get(&key)`, `.values()`, `.len()`, etc. directly.
    pub fn read(&self) -> MutableBTreeMapLockRef<'_, String, T> {
        self.items.lock_ref()
    }

    /// Get a reactive signal map that emits `MapDiff` on every change.
    ///
    /// The signal emits an initial `MapDiff::Replace` with all current entries,
    /// followed by `MapDiff::Insert` / `MapDiff::Update` / `MapDiff::Remove`
    /// for each subsequent change.
    ///
    /// Use [`SignalMapExt::for_each`] (or [`SignalMapExt::key_cloned`]) to react
    /// to changes in a background task.
    pub fn signal_map(&self) -> MutableSignalMap<String, T> {
        self.items.signal_map_cloned()
    }

    /// Insert or update an item directly (bypasses CDC).
    ///
    /// Use this for optimistic cache updates after a write to ensure
    /// the LiveData is immediately consistent, without waiting for the
    /// CDC roundtrip through matview → stream → apply_changes.
    pub fn insert(&self, key: String, value: T) {
        self.items.lock_mut().insert_cloned(key, value);
    }

    /// Apply a batch of CDC changes incrementally.
    pub fn apply_changes(&self, changes: Vec<RowChange>) {
        let mut lock = self.items.lock_mut();
        let mut rowid_map = self
            .rowid_to_key
            .lock()
            .expect("rowid_to_key mutex poisoned");
        for rc in changes {
            match rc.change {
                Change::Created { data, .. } | Change::Updated { data, .. } => {
                    let id = (self.id_fn)(&data).expect("id_fn failed on CDC row");
                    let parsed = (self.parse_fn)(&data).expect("parse_fn failed on CDC row");
                    if let Some(rowid) = extract_rowid(&data) {
                        rowid_map.insert(rowid, id.clone());
                    }
                    lock.insert_cloned(id, parsed);
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
                    panic!("LiveData: unexpected FieldsChanged for {entity_id}");
                }
            }
        }
    }

    /// Spawn a background task that listens to the CDC stream and applies changes.
    ///
    /// Each batch's `metadata.seq` is recorded after `apply_changes` returns,
    /// so callers can `wait_for_seq` until the mirror has caught up to a
    /// specific CDC watermark.
    pub fn subscribe(self: &Arc<Self>, mut stream: crate::storage::turso::RowChangeStream) {
        let live = Arc::clone(self);
        crate::util::spawn_actor(async move {
            use tokio_stream::StreamExt;
            while let Some(batch) = stream.next().await {
                let seq = batch.metadata.seq;
                let changes: Vec<RowChange> = batch.inner.items.into_iter().collect();
                live.apply_changes(changes);
                if seq > 0 {
                    live.last_consumed_seq.store(seq, Ordering::SeqCst);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::task::{Context, Poll};

    use futures_signals::signal_map::{MapDiff, SignalMapExt};
    use holon_api::Value;

    fn make_row(id: &str, content: &str) -> StorageEntity {
        let mut row = HashMap::new();
        row.insert("id".to_string(), Value::String(id.to_string()));
        row.insert("content".to_string(), Value::String(content.to_string()));
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
        assert_eq!(items.get("a").unwrap(), "hello");
        assert_eq!(items.get("b").unwrap(), "world");
    }

    #[test]
    fn test_apply_changes() {
        let live: Arc<LiveData<String>> = LiveData::new(
            vec![],
            |row| Ok(row.get("id").unwrap().as_string().unwrap().to_string()),
            |row| Ok(row.get("content").unwrap().as_string().unwrap().to_string()),
        );

        let created = RowChange {
            relation_name: "test".to_string(),
            change: Change::Created {
                data: make_row("c", "new"),
                origin: holon_api::ChangeOrigin::Local {
                    operation_id: None,
                    trace_id: None,
                },
            },
        };
        live.apply_changes(vec![created]);

        let items = live.read();
        assert_eq!(items.get("c").unwrap(), "new");
    }

    #[test]
    fn test_apply_delete() {
        let live: Arc<LiveData<String>> = LiveData::new(
            vec![make_row("x", "data")],
            |row| Ok(row.get("id").unwrap().as_string().unwrap().to_string()),
            |row| Ok(row.get("content").unwrap().as_string().unwrap().to_string()),
        );

        assert_eq!(live.read().len(), 1);

        let deleted = RowChange {
            relation_name: "test".to_string(),
            change: Change::Deleted {
                id: "x".to_string(),
                origin: holon_api::ChangeOrigin::Local {
                    operation_id: None,
                    trace_id: None,
                },
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
        let mut row = HashMap::new();
        row.insert(
            "region".to_string(),
            Value::String("right_sidebar".to_string()),
        );
        row.insert(
            "root_id".to_string(),
            Value::String("block:abc".to_string()),
        );
        row.insert("_rowid".to_string(), Value::String("42".to_string()));

        live.apply_changes(vec![RowChange {
            relation_name: "focus_roots".to_string(),
            change: Change::Created {
                data: row,
                origin: holon_api::ChangeOrigin::Local {
                    operation_id: None,
                    trace_id: None,
                },
            },
        }]);

        assert_eq!(live.read().len(), 1, "Created should have inserted 1 row");

        // CDC Delete for the same matview row. `process_cdc_event` falls back
        // to `change.id.to_string()` (rowid) because data has no `id` column.
        live.apply_changes(vec![RowChange {
            relation_name: "focus_roots".to_string(),
            change: Change::Deleted {
                id: "42".to_string(),
                origin: holon_api::ChangeOrigin::Local {
                    operation_id: None,
                    trace_id: None,
                },
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
                assert_eq!(entries[0].1, "seeded");
            }
            other => panic!("Expected Ready(Some(Replace)), got {:?}", other),
        }

        // Apply a CDC change
        live.apply_changes(vec![RowChange {
            relation_name: "test".to_string(),
            change: Change::Created {
                data: make_row("a", "new-item"),
                origin: holon_api::ChangeOrigin::Local {
                    operation_id: None,
                    trace_id: None,
                },
            },
        }]);

        // Second poll: MapDiff::Insert for the new entry
        match signal.poll_map_change_unpin(&mut cx) {
            Poll::Ready(Some(MapDiff::Insert { key, value })) => {
                assert_eq!(key, "a");
                assert_eq!(value, "new-item");
            }
            other => panic!("Expected Ready(Some(Insert)), got {:?}", other),
        }
    }
}
