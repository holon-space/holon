//! SQL-backed persistence and live-state reader for the undo substrate.
//!
//! - [`SqlUndoStore`] persists the whole [`UndoStack`](holon_core::UndoStack)
//!   as a single compacted JSON snapshot in an `undo_log` table (one row per
//!   replica DB), so history survives a restart. A per-entry log would be an
//!   equivalent, heavier encoding; the snapshot is bounded by the stack's
//!   `max_size` and re-verified at replay, which is what makes it safe.
//! - [`SqlUndoStateReader`] reads the current projected value of an (entity,
//!   field) from the block write table so a stored
//!   [`Precondition`](holon_core::Precondition) can be checked against live
//!   state before an inverse replays (staleness policy).
//!
//! Only the `block` entity carries real inverses today, so the reader targets
//! the block write table; other entities are `DeclaredIrreversible` and never
//! produce a precondition to verify.

use std::collections::HashMap;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use holon_api::Value;
use holon_core::UndoStateReader;
use holon_core::UndoStore;

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

/// Read current (entity, field) values from the block write table for staleness
/// verification.
pub struct SqlUndoStateReader {
    db: DbHandle,
    table: String,
}

impl SqlUndoStateReader {
    pub fn new(db: DbHandle, table: impl Into<String>) -> Self {
        Self {
            db,
            table: table.into(),
        }
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
    async fn field_value(&self, entity_id: &str, field: &str) -> Result<Option<Value>> {
        assert_identifier(field, "field")?;
        assert_identifier(&self.table, "table")?;
        let escaped = entity_id.replace('\'', "''");
        let sql = format!("SELECT {field} FROM {} WHERE id = '{escaped}'", self.table);
        let rows = self
            .db
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| anyhow::anyhow!("undo reader query: {e}"))?;
        Ok(rows.first().and_then(|r| r.get(field)).cloned())
    }
}
