//! CDC-aware self-updating collection keyed by entity ID.
//!
//! Uses `futures_signals::signal_map::MutableBTreeMap` for reactive storage:
//! change events are broadcast natively as `MapDiff` values — no separate
//! version channel needed.

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;
use futures_signals::signal_map::{MutableBTreeMap, MutableBTreeMapLockRef, MutableSignalMap};
use holon_api::Change;

use crate::storage::turso::RowChange;
use crate::storage::types::StorageEntity;

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
}

impl<T: Clone + Send + Sync + 'static> LiveData<T> {
    pub fn new(
        initial_rows: Vec<StorageEntity>,
        id_fn: impl Fn(&StorageEntity) -> Result<String> + Send + Sync + 'static,
        parse_fn: impl Fn(&StorageEntity) -> Result<T> + Send + Sync + 'static,
    ) -> Arc<Self> {
        let mut items = BTreeMap::new();
        for row in initial_rows {
            let id = (id_fn)(&row).expect("id_fn failed on initial row");
            let parsed = (parse_fn)(&row).expect("parse_fn failed on initial row");
            items.insert(id, parsed);
        }

        Arc::new(Self {
            items: MutableBTreeMap::with_values(items),
            id_fn: Box::new(id_fn),
            parse_fn: Box::new(parse_fn),
        })
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
        for rc in changes {
            match rc.change {
                Change::Created { data, .. } | Change::Updated { data, .. } => {
                    let id = (self.id_fn)(&data).expect("id_fn failed on CDC row");
                    let parsed = (self.parse_fn)(&data).expect("parse_fn failed on CDC row");
                    lock.insert_cloned(id, parsed);
                }
                Change::Deleted { id, .. } => {
                    lock.remove(&id);
                }
                Change::FieldsChanged { entity_id, .. } => {
                    panic!("LiveData: unexpected FieldsChanged for {entity_id}");
                }
            }
        }
    }

    /// Spawn a background task that listens to the CDC stream and applies changes.
    pub fn subscribe(self: &Arc<Self>, mut stream: crate::storage::turso::RowChangeStream) {
        let live = Arc::clone(self);
        crate::util::spawn_actor(async move {
            use tokio_stream::StreamExt;
            while let Some(batch) = stream.next().await {
                let changes: Vec<RowChange> = batch.inner.items.into_iter().collect();
                live.apply_changes(changes);
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
