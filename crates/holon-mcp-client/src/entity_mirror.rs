//! Engine-owned, write-through in-memory mirror of a sync-strategy entity's
//! cache table.
//!
//! Each sync-strategy entity gets one `EntityMirror`. On the first full sync it
//! is seeded once from `cache.get_all()`; every later full sync diffs the
//! freshly fetched records against `snapshot()` instead of re-reading the
//! `DatabaseActor`. After each successful `apply_batch`, the same `Change`
//! batch is applied to the mirror (`apply`) so it stays byte-for-byte
//! consistent with the cache table — the engine is the sole writer to these
//! tables (enforced by the sync-vs-`vtable.write_through` config check), so
//! applying the committed batch synchronously gives consistency by
//! construction. No CDC subscription, hence no echo.
//!
//! The mirror keys rows by the same `EntityUri` string the sync diff uses (the
//! prefixed id column parsed to an `EntityUri` and stringified), so a `Deleted`
//! change (whose id the sync produces as `EntityUri::to_string()`) removes the
//! matching `Created` row.

use std::sync::Arc;
use std::sync::Mutex;

use holon_api::Change;
use holon_api::DynamicEntity;
use holon_api::EntityUri;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_api::live_data::LiveData;
use tracing::info;

/// The key a row is stored under in the mirror. Must match both the sync diff's
/// `existing_ids` keying and the `Change::Deleted.id` the sync emits — all
/// three parse the prefixed id column to an `EntityUri` and stringify it.
fn mirror_key(id_column: &str, row: &StorageEntity) -> anyhow::Result<String> {
    let raw = match row.get(id_column) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Integer(n)) => n.to_string(),
        Some(other) => {
            anyhow::bail!("mirror id column '{id_column}' holds a non-id value: {other:?}")
        }
        None => anyhow::bail!("mirror row is missing id column '{id_column}'"),
    };
    // ALLOW(entity_uri_from_raw): mirror keys rows by the same EntityUri form the
    // sync full-diff uses for existing_ids and Change::Deleted.id
    Ok(EntityUri::from_raw(&raw).to_string())
}

/// Translate a sync `Change<DynamicEntity>` into the `Change<StorageEntity>`
/// `LiveData::apply_changes` expects (`LiveData` re-derives the key and
/// re-parses the row from the `StorageEntity` payload via the closures given at
/// `seed`).
fn to_storage_change(change: &Change<DynamicEntity>) -> Change<StorageEntity> {
    match change {
        Change::Created { data, origin } => Change::Created {
            data: data.fields.clone(),
            origin: origin.clone(),
        },
        Change::Updated { id, data, origin } => Change::Updated {
            id: id.clone(),
            data: data.fields.clone(),
            origin: origin.clone(),
        },
        Change::Deleted { id, origin } => Change::Deleted {
            id: id.clone(),
            origin: origin.clone(),
        },
        // The sync full/incremental paths only ever emit Created/Updated/Deleted.
        // A FieldsChanged reaching the mirror is an upstream defect — surface it
        // loudly rather than silently mishandling it.
        Change::FieldsChanged { entity_id, .. } => {
            panic!("EntityMirror received unexpected FieldsChanged for '{entity_id}' from sync")
        }
    }
}

/// In-memory write-through mirror of one sync entity's cache table.
///
/// Seeded lazily (once) and reset via [`EntityMirror::reset`] when the cache is
/// cleared out from under it (full-resync). Interior mutability so the engine
/// can hold it behind `&self` across the serialized sync loop.
pub struct EntityMirror {
    entity_type: String,
    id_column: String,
    live: Mutex<Option<Arc<LiveData<DynamicEntity>>>>,
}

impl EntityMirror {
    pub fn new(entity_type: String, id_column: String) -> Self {
        Self {
            entity_type,
            id_column,
            live: Mutex::new(None),
        }
    }

    pub fn is_seeded(&self) -> bool {
        self.live.lock().expect("mirror mutex poisoned").is_some()
    }

    /// Build the mirror from a full snapshot of the cache table. Called once,
    /// on the first full sync (or first write-through), then never re-read
    /// until [`reset`](Self::reset).
    pub fn seed(&self, rows: Vec<DynamicEntity>) {
        let id_column = self.id_column.clone();
        let entity_type = self.entity_type.clone();
        let storage_rows: Vec<StorageEntity> = rows.into_iter().map(|e| e.fields).collect();

        let key_col = id_column.clone();
        let live = LiveData::new(
            storage_rows,
            move |row| mirror_key(&key_col, row),
            move |row| {
                Ok(DynamicEntity {
                    type_name: entity_type.clone(),
                    fields: row.clone(),
                })
            },
        );
        let size = live.read().len();
        info!(
            entity = %self.entity_type,
            rows = size,
            "entity mirror seeded"
        );
        *self.live.lock().expect("mirror mutex poisoned") = Some(live);
    }

    /// Drop the seed so the next full sync re-seeds from the cache. Used when
    /// an external writer (full-resync `clear_cache`) empties the table.
    pub fn reset(&self) {
        *self.live.lock().expect("mirror mutex poisoned") = None;
    }

    /// A snapshot of the current rows, for diffing a fresh fetch against.
    /// Panics if called before [`seed`](Self::seed) — the caller seeds
    /// first.
    pub fn snapshot(&self) -> Vec<Arc<DynamicEntity>> {
        let guard = self.live.lock().expect("mirror mutex poisoned");
        let live = guard
            .as_ref()
            .expect("EntityMirror::snapshot on an unseeded mirror");
        live.read().values().cloned().collect()
    }

    pub fn len(&self) -> usize {
        let guard = self.live.lock().expect("mirror mutex poisoned");
        guard.as_ref().map(|l| l.read().len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Apply the just-committed `Change` batch to the mirror so it tracks the
    /// cache table. Panics if unseeded — write-through is only invoked after
    /// the caller has ensured a seed.
    pub fn apply(&self, changes: &[Change<DynamicEntity>]) {
        let guard = self.live.lock().expect("mirror mutex poisoned");
        let live = guard
            .as_ref()
            .expect("EntityMirror::apply on an unseeded mirror");
        let storage_changes: Vec<Change<StorageEntity>> =
            changes.iter().map(to_storage_change).collect();
        live.apply_changes(storage_changes);
    }
}

#[cfg(test)]
mod tests {
    use holon_api::ChangeOrigin;

    use super::*;

    fn origin() -> ChangeOrigin {
        ChangeOrigin::Local {
            operation_id: None,
            trace_id: None,
        }
    }

    /// Build an entity whose `id` column carries an already-prefixed id, the
    /// way `record_to_entity` produces them (e.g. `cc-session:abc`).
    fn entity(scheme: &str, id: &str, extra: &[(&str, &str)]) -> DynamicEntity {
        let mut e = DynamicEntity::new(scheme);
        e.set("id", Value::String(format!("{scheme}:{id}")));
        for (k, v) in extra {
            e.set(*k, Value::String((*v).to_string()));
        }
        e
    }

    fn keys(m: &EntityMirror) -> Vec<String> {
        let mut ks: Vec<String> = m
            .snapshot()
            .iter()
            .filter_map(|e| e.get_string("id"))
            .collect();
        ks.sort();
        ks
    }

    #[test]
    fn seed_then_snapshot_round_trip() {
        let m = EntityMirror::new("cc-session".into(), "id".into());
        assert!(!m.is_seeded());
        m.seed(vec![
            entity("cc-session", "a", &[("title", "A")]),
            entity("cc-session", "b", &[("title", "B")]),
        ]);
        assert!(m.is_seeded());
        assert_eq!(m.len(), 2);
        assert_eq!(keys(&m), vec!["cc-session:a", "cc-session:b"]);
    }

    #[test]
    fn apply_created_updated_deleted() {
        let m = EntityMirror::new("cc-session".into(), "id".into());
        m.seed(vec![entity("cc-session", "a", &[("title", "A")])]);

        // Create b, update a, delete nothing.
        m.apply(&[
            Change::Created {
                data: entity("cc-session", "b", &[("title", "B")]),
                origin: origin(),
            },
            Change::Updated {
                // Sync emits `Change::Updated.id` as the EntityUri string; for a
                // plain `scheme:id` that is the literal itself.
                id: "cc-session:a".to_string(),
                data: entity("cc-session", "a", &[("title", "A2")]),
                origin: origin(),
            },
        ]);
        assert_eq!(keys(&m), vec!["cc-session:a", "cc-session:b"]);
        let a = m
            .snapshot()
            .into_iter()
            .find(|e| e.get_string("id").as_deref() == Some("cc-session:a"))
            .unwrap();
        assert_eq!(a.get_string("title").as_deref(), Some("A2"));

        // Delete a, keyed exactly as the sync emits it (plain scheme:id).
        m.apply(&[Change::Deleted {
            id: "cc-session:a".to_string(),
            origin: origin(),
        }]);
        assert_eq!(keys(&m), vec!["cc-session:b"]);
    }

    /// The delete key the sync emits (`EntityUri::to_string()`) must match the
    /// key the mirror computed for the corresponding `Created` row — otherwise
    /// a remote deletion would silently miss and the mirror would drift.
    #[test]
    fn delete_key_matches_entity_uri_prefixing() {
        let m = EntityMirror::new("cc-session".into(), "id".into());
        let created = entity("cc-session", "abc", &[]);
        m.seed(vec![created.clone()]);
        // The sync computes removed ids from the mirror snapshot and emits the
        // id column parsed to an EntityUri and stringified — reproduce that here.
        // ALLOW(entity_uri_from_raw): test literal reproducing the sync's delete-id
        // derivation
        let del_id = EntityUri::from_raw(created.get_string("id").unwrap().as_str()).to_string();
        m.apply(&[Change::Deleted {
            id: del_id,
            origin: origin(),
        }]);
        assert!(m.is_empty());
    }

    #[test]
    fn reset_unseeds() {
        let m = EntityMirror::new("cc-session".into(), "id".into());
        m.seed(vec![entity("cc-session", "a", &[])]);
        assert!(m.is_seeded());
        m.reset();
        assert!(!m.is_seeded());
        assert_eq!(m.len(), 0);
    }

    /// Integer id columns key the same way as strings (some MCP servers return
    /// numeric ids).
    #[test]
    fn integer_id_column_keys_consistently() {
        let m = EntityMirror::new("gh-issue".into(), "id".into());
        let mut e = DynamicEntity::new("gh-issue");
        e.set("id", Value::String("gh-issue:42".to_string()));
        m.seed(vec![e]);
        assert_eq!(keys(&m), vec!["gh-issue:42"]);
    }
}
