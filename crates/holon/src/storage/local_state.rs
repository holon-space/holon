//! Local, non-syncing UI state — the `local_ui_state` table behind
//! [`LocalStateStore`] (C8 ruling, 2026-07-13).
//!
//! Per-device view choices (which perspective this DEVICE shows, a local
//! view-mode override) live in a **local-only Turso table**, NEVER as extra
//! columns/rows on the replicated block tables: those are Loro projections
//! under ADR 0025 — sink-truth diffing, reseeds, and the conformance
//! tripwires all assume op-grounded content, so un-grounded rows there get
//! wiped or trip conformance. `local_ui_state` is outside every projection /
//! reseed path (the sink reconciles only the block sink tables), so local
//! state survives reseeds and reboots over the same DB.
//!
//! **Disclosed loss on DB rebuild**: deleting/rebuilding the replica DB loses
//! local UI state. This is consistent with the C2b "Turso = disclosed
//! ephemeral cache" doctrine — synced state (block properties in Loro) is the
//! durable tier; local overrides are per-device by definition.
//!
//! **Precedence is resolved IN the slot query**, not in code:
//! `COALESCE(local override, synced choice, default)` — so a local override
//! wins until cleared, and remote-wins stays expressible by reordering the
//! COALESCE arms. See `BlockDomain::load_perspective_blocks` for the root
//! display slot's use.

use std::collections::HashMap;

use anyhow::Context;
use anyhow::Result;
use holon_api::EntityUri;
use holon_api::Value;

use crate::storage::DbHandle;

/// The local-only table name. Referenced by slot queries (COALESCE arms).
pub const LOCAL_UI_STATE_TABLE: &str = "local_ui_state";

/// Ensure the `local_ui_state` table exists. Idempotent.
pub async fn ensure_local_ui_state(db: &DbHandle) -> Result<()> {
    db.execute_ddl(
        "CREATE TABLE IF NOT EXISTS local_ui_state (scope_block_id TEXT NOT NULL, key TEXT NOT \
         NULL, value TEXT NOT NULL, PRIMARY KEY (scope_block_id, key))",
    )
    .await
    .context("create local_ui_state table")?;
    Ok(())
}

/// Read/write handle for local, non-syncing UI state.
///
/// Keys are scoped to a block (`scope_block_id`) so the same key (e.g.
/// `active_perspective`, `view_mode`) can carry a different local choice per
/// scope. Values are plain strings — the consuming query parses them at its
/// boundary (fail loud there, not here).
pub struct LocalStateStore {
    db: DbHandle,
}

impl LocalStateStore {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }

    /// Set (upsert) a local override.
    pub async fn set(&self, scope_block_id: &EntityUri, key: &str, value: &str) -> Result<()> {
        self.db
            .execute_values(
                "INSERT INTO local_ui_state (scope_block_id, key, value) VALUES (?, ?, ?) ON \
                 CONFLICT(scope_block_id, key) DO UPDATE SET value = excluded.value",
                vec![
                    Value::String(scope_block_id.to_string()),
                    Value::String(key.to_string()),
                    Value::String(value.to_string()),
                ],
            )
            .await
            .map_err(|e| anyhow::anyhow!("local_ui_state set({scope_block_id}, {key}): {e}"))?;
        Ok(())
    }

    /// Read a local override, `None` when no override is set.
    pub async fn get(&self, scope_block_id: &EntityUri, key: &str) -> Result<Option<String>> {
        let mut params = HashMap::new();
        params.insert(
            "scope".to_string(),
            Value::String(scope_block_id.to_string()),
        );
        params.insert("key".to_string(), Value::String(key.to_string()));
        let rows = self
            .db
            .query(
                "SELECT value FROM local_ui_state WHERE scope_block_id = $scope AND key = $key",
                params,
            )
            .await
            .map_err(|e| anyhow::anyhow!("local_ui_state get({scope_block_id}, {key}): {e}"))?;
        Ok(rows
            .first()
            .and_then(|r| r.get("value"))
            .and_then(Value::as_string_owned))
    }

    /// Clear a local override — the synced choice (or default) takes over via
    /// the COALESCE precedence in the consuming query.
    pub async fn clear(&self, scope_block_id: &EntityUri, key: &str) -> Result<()> {
        let mut params = HashMap::new();
        params.insert(
            "scope".to_string(),
            Value::String(scope_block_id.to_string()),
        );
        params.insert("key".to_string(), Value::String(key.to_string()));
        self.db
            .query(
                "DELETE FROM local_ui_state WHERE scope_block_id = $scope AND key = $key",
                params,
            )
            .await
            .map_err(|e| anyhow::anyhow!("local_ui_state clear({scope_block_id}, {key}): {e}"))?;
        Ok(())
    }
}
