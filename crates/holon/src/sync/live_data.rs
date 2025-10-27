//! CDC-aware self-updating collection keyed by entity ID.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock, RwLockReadGuard};

use anyhow::Result;
use holon_api::Change;

use crate::storage::turso::RowChange;
use crate::storage::types::StorageEntity;

/// A live, CDC-driven collection of items keyed by entity ID.
///
/// `T` is the item type. Items are parsed from `StorageEntity` (HashMap<String, Value>)
/// via the `parse_fn` provided at construction.
/// CDC events (Created/Updated/Deleted) are applied incrementally.
///
/// Both `id_fn` and `parse_fn` return `Result` — if they fail, it's a programming
/// error (wrong table, schema mismatch) and should be loud, not silently swallowed.
pub struct LiveData<T: Send + Sync + 'static> {
    items: RwLock<HashMap<String, T>>,
    version: AtomicU64,
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

        Arc::new(Self {
            items: RwLock::new(items),
            version: AtomicU64::new(1),
            id_fn: Box::new(id_fn),
            parse_fn: Box::new(parse_fn),
        })
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    pub fn read(&self) -> RwLockReadGuard<'_, HashMap<String, T>> {
        self.items.read().unwrap()
    }

    /// Apply a batch of CDC changes incrementally.
    pub fn apply_changes(&self, changes: Vec<RowChange>) {
        let mut items = self.items.write().unwrap();
        for rc in changes {
            match rc.change {
                Change::Created { data, .. } | Change::Updated { data, .. } => {
                    let id = (self.id_fn)(&data).expect("id_fn failed on CDC row");
                    let parsed = (self.parse_fn)(&data).expect("parse_fn failed on CDC row");
                    items.insert(id, parsed);
                }
                Change::Deleted { id, .. } => {
                    items.remove(&id);
                }
                Change::FieldsChanged { entity_id, .. } => {
                    // Matview CDC emits Created/Deleted (not FieldsChanged).
                    panic!("LiveData: unexpected FieldsChanged for {entity_id}");
                }
            }
        }
        self.version.fetch_add(1, Ordering::Release);
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
}
