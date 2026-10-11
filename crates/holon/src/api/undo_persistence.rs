//! SQL-backed persistence and live-state reader for the undo substrate.
//!
//! - [`SqlUndoStore`] persists the whole [`UndoStack`](holon_core::UndoStack)
//!   as a single compacted JSON snapshot in an `undo_log` table (one row per
//!   replica DB), so history survives a restart. A per-entry log would be an
//!   equivalent, heavier encoding; the snapshot is bounded by the stack's
//!   `max_size` and re-verified at replay, which is what makes it safe.
//! - [`SqlUndoStateReader`] reads the current projected value of an (entity,
//!   field) from the write table of the entity's type, chosen by its URI
//!   scheme, so a stored [`Precondition`](holon_core::Precondition) can be
//!   checked against live state before an inverse replays (staleness policy).

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use holon_api::EntityUri;
use holon_api::Value;
use holon_core::UndoStateReader;
use holon_core::UndoStore;
use holon_profiles::TypeRegistry;
use holon_turso::turso_adapter::TursoAdapter;

use crate::storage::DbHandle;

/// Fixed single-row key for the snapshot.
const SNAPSHOT_ID: i64 = 0;

/// Ensure the `undo_log` table exists. Idempotent.
pub async fn ensure_undo_log(db: &DbHandle) -> Result<()> {
    db.execute_ddl(
        "CREATE TABLE IF NOT EXISTS undo_log (id INTEGER PRIMARY KEY, seq INTEGER NOT NULL, \
         state_json TEXT NOT NULL)",
    )
    .await
    .context("create undo_log table")?;
    Ok(())
}

/// Persist/load the undo stack snapshot in `undo_log`.
pub struct SqlUndoStore {
    db: DbHandle,
}

impl SqlUndoStore {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl UndoStore for SqlUndoStore {
    async fn load(&self) -> Result<Option<String>> {
        let rows = self
            .db
            .query(
                &format!("SELECT state_json FROM undo_log WHERE id = {SNAPSHOT_ID}"),
                HashMap::new(),
            )
            .await
            .map_err(|e| anyhow::anyhow!("undo_log load: {e}"))?;
        Ok(rows
            .first()
            .and_then(|r| r.get("state_json"))
            .and_then(Value::as_string_owned))
    }

    async fn save(&self, state_json: &str, seq: i64) -> Result<()> {
        // Single-row upsert: full-snapshot compaction.
        self.db
            .execute_values(
                "INSERT INTO undo_log (id, seq, state_json) VALUES (?, ?, ?) ON CONFLICT(id) DO \
                 UPDATE SET seq = excluded.seq, state_json = excluded.state_json",
                vec![
                    Value::Integer(SNAPSHOT_ID),
                    Value::Integer(seq),
                    Value::String(state_json.to_string()),
                ],
            )
            .await
            .map_err(|e| anyhow::anyhow!("undo_log save: {e}"))?;
        Ok(())
    }
}

/// Read current (entity, field) values from the write table of the entity's
/// type for staleness verification.
pub struct SqlUndoStateReader {
    db: DbHandle,
    block_table: String,
    types: Arc<TypeRegistry>,
}

impl SqlUndoStateReader {
    pub fn new(db: DbHandle, block_table: impl Into<String>, types: Arc<TypeRegistry>) -> Self {
        Self {
            db,
            block_table: block_table.into(),
            types,
        }
    }

    fn table_of(&self, entity: &EntityUri) -> Result<String> {
        if entity.is_block() {
            return Ok(self.block_table.clone());
        }
        let type_def = self.types.get(entity.scheme()).with_context(|| {
            format!("undo reader: no declared type owns the scheme of {entity}")
        })?;
        Ok(TursoAdapter::raw_table_name(&type_def))
    }
}

/// A column/table identifier must be a bare SQL identifier — reject anything
/// that could break out of the inlined query (fail loud, never silently read
/// the wrong thing).
fn assert_identifier(name: &str, kind: &str) -> Result<()> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        anyhow::bail!("undo reader: invalid {kind} identifier {name:?}");
    }
    Ok(())
}

#[async_trait]
impl UndoStateReader for SqlUndoStateReader {
    async fn field_value(&self, entity: &EntityUri, field: &str) -> Result<Option<Value>> {
        let table = self.table_of(entity)?;
        assert_identifier(field, "field")?;
        assert_identifier(&table, "table")?;
        // The write table keys its rows by the SCHEME-QUALIFIED id, which is
        // what `EntityUri::as_str` renders. This is the one place the id
        // becomes SQL text.
        let escaped = entity.as_str().replace('\'', "''");
        let sql = format!("SELECT {field} FROM {table} WHERE id = '{escaped}'");
        let rows = self
            .db
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| anyhow::anyhow!("undo reader query: {e}"))?;
        Ok(rows.first().and_then(|r| r.get(field)).cloned())
    }
}
