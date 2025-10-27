//! CDC-aware self-updating collection keyed by entity ID.
//!
//! Uses `tokio::sync::watch` for push-based version notification (no polling)
//! and delegates CDC change application to `MapDiff` from `holon_api::reactive`.

use std::collections::HashMap;
use std::sync::{Arc, RwLock, RwLockReadGuard};

use anyhow::Result;
use holon_api::Change;
use holon_api::reactive::MapDiff;

use crate::storage::turso::RowChange;
use crate::storage::types::StorageEntity;

/// A live, CDC-driven collection of items keyed by entity ID.
///
/// `T` is the item type. Items are parsed from `StorageEntity` (HashMap<String, Value>)
/// via the `parse_fn` provided at construction.
/// CDC events (Created/Updated/Deleted) are applied incrementally.
///
/// Version changes are broadcast via `tokio::sync::watch` — consumers can use
/// `subscribe_version()` for push-based cache invalidation instead of polling.
///
/// Both `id_fn` and `parse_fn` return `Result` — if they fail, it's a programming
/// error (wrong table, schema mismatch) and should be loud, not silently swallowed.
pub struct LiveData<T: Send + Sync + 'static> {
    items: RwLock<HashMap<String, T>>,
    version_tx: tokio::sync::watch::Sender<u64>,
    id_fn: Box<dyn Fn(&StorageEntity) -> Result<String> + Send + Sync>,
    parse_fn: Box<dyn Fn(&StorageEntity) -> Result<T> + Send + Sync>,
}

impl<T: Send + Sync + 'static> LiveData<T> {
    pub fn new(
        initial_rows: Vec<StorageEntity>,
        id_fn: impl Fn(&StorageEntity) -> Result<String> + Send + Sync + 'static,
        parse_fn: impl Fn(&StorageEntity) -> Result<T> + Send + Sync + 'static,
    ) -> Arc<Self> {
        let mut items = HashMap::new();
        for row in initial_rows {
            let id = (id_fn)(&row).expect("id_fn failed on initial row");
            let parsed = (parse_fn)(&row).expect("parse_fn failed on initial row");
            items.insert(id, parsed);
        }

        let (version_tx, _) = tokio::sync::watch::channel(1u64);

        Arc::new(Self {
            items: RwLock::new(items),
            version_tx,
            id_fn: Box::new(id_fn),
            parse_fn: Box::new(parse_fn),
        })
    }

    /// Current version number (incremented on each batch of changes).
    pub fn version(&self) -> u64 {
        *self.version_tx.borrow()
    }

    /// Read the current snapshot. Returns a guard — hold briefly.
    pub fn read(&self) -> RwLockReadGuard<'_, HashMap<String, T>> {
        self.items.read().unwrap()
    }

    /// Get a `watch::Receiver` that is notified on every version change.
    ///
    /// Use `rx.changed().await` for push-based cache invalidation instead of
    /// polling `version()`.
    pub fn subscribe_version(&self) -> tokio::sync::watch::Receiver<u64> {
        self.version_tx.subscribe()
    }

    /// Insert or update an item directly (bypasses CDC).
    ///
    /// Use this for optimistic cache updates after a write to ensure
    /// the LiveData is immediately consistent, without waiting for the
    /// CDC roundtrip through matview → stream → apply_changes.
    pub fn insert(&self, key: String, value: T) {
        self.items.write().unwrap().insert(key, value);
        self.version_tx.send_modify(|v| *v += 1);
    }

    /// Apply a batch of CDC changes incrementally.
    pub fn apply_changes(&self, changes: Vec<RowChange>) {
        let mut items = self.items.write().unwrap();
        for rc in changes {
            let diff = self.row_change_to_diff(rc);
            holon_api::reactive::apply_map_diff(&mut items, diff);
        }
        // Bump version and notify watchers
        self.version_tx.send_modify(|v| *v += 1);
    }

    /// Convert a RowChange into a MapDiff, parsing the entity via id_fn/parse_fn.
    fn row_change_to_diff(&self, rc: RowChange) -> MapDiff<String, T> {
        match rc.change {
            Change::Created { data, .. } | Change::Updated { data, .. } => {
                let id = (self.id_fn)(&data).expect("id_fn failed on CDC row");
                let parsed = (self.parse_fn)(&data).expect("parse_fn failed on CDC row");
                MapDiff::Insert {
                    key: id,
                    value: parsed,
                }
            }
            Change::Deleted { id, .. } => MapDiff::Remove { key: id },
            Change::FieldsChanged { entity_id, .. } => {
                // Matview CDC emits Created/Deleted (not FieldsChanged).
                panic!("LiveData: unexpected FieldsChanged for {entity_id}");
            }
        }
    }

    /// Spawn a background task that listens to the CDC stream and applies changes.
    pub fn subscribe(self: &Arc<Self>, mut stream: crate::storage::turso::RowChangeStream) {
        let live = Arc::clone(self);
        tokio::spawn(async move {
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

        assert_eq!(live.version(), 1);
        let items = live.read();
        assert_eq!(items.len(), 2);
        assert_eq!(items.get("a").unwrap(), "hello");
        assert_eq!(items.get("b").unwrap(), "world");
    }

    #[test]
    fn test_apply_changes_increments_version() {
        let live: Arc<LiveData<String>> = LiveData::new(
            vec![],
            |row| Ok(row.get("id").unwrap().as_string().unwrap().to_string()),
            |row| Ok(row.get("content").unwrap().as_string().unwrap().to_string()),
        );

        assert_eq!(live.version(), 1);

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

        assert_eq!(live.version(), 2);
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
    fn test_subscribe_version_notifies() {
        let live: Arc<LiveData<String>> = LiveData::new(
            vec![],
            |row| Ok(row.get("id").unwrap().as_string().unwrap().to_string()),
            |row| Ok(row.get("content").unwrap().as_string().unwrap().to_string()),
        );

        let mut rx = live.subscribe_version();
        assert_eq!(*rx.borrow(), 1);

        live.apply_changes(vec![RowChange {
            relation_name: "test".to_string(),
            change: Change::Created {
                data: make_row("a", "test"),
                origin: holon_api::ChangeOrigin::Local {
                    operation_id: None,
                    trace_id: None,
                },
            },
        }]);

        // rx.has_changed() should be true
        assert!(rx.has_changed().unwrap());
        assert_eq!(*rx.borrow_and_update(), 2);
    }
}
