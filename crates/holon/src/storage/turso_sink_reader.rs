//! Production [`SinkReader`] over the Turso SQL sink.
//!
//! Lives in the storage layer (Phase 2.5, C4 of the architecture plan): the
//! consolidation layer (`sync/loro_sync_controller.rs`) defines the
//! [`SinkReader`] seam and must not import concrete Turso types; this concrete
//! reader is constructed at wiring time (`sync/loro_module.rs`).

use std::collections::HashMap;

use anyhow::Result;
use holon_api::block::Block;
use holon_core::SinkReader;
use holon_core::fractional_index::default_sort_key;

use crate::api::SnapshotBlock;
use crate::storage::BLOCK_WRITE_TABLE;
use crate::storage::turso::DbHandle;

/// Production [`SinkReader`]: reads the `block_raw` base table directly — NOT
/// the `block` matview, which can lag `block_raw` under IVM and would make the
/// projection see stale state and re-emit redundant writes. `tags`/`requires`
/// are hydrated from their junction tables; both are part of the block
/// equivalence relation (`blocks_differ` iterates `EdgeField::ALL`).
///
/// The self-parented `sentinel:no_parent` FK-anchor row is EXCLUDED: it is
/// schema infrastructure, not a domain block, and it never exists in Loro. If
/// it leaks into the diff base, every armed reseed pass diffs it against the
/// Loro snapshot and emits `delete('sentinel:no_parent')` — `prepare_delete`
/// then fails loud on the self-parent cycle, the pass errors, `seeded` stays
/// false, and the projector wedges in a permanent error/reseed storm (the
/// dogfooded per-transition "withholding 1 delete(s)" WARN is the same row,
/// caught by the unarmed gate).
pub struct TursoSinkReader {
    db_handle: DbHandle,
}

impl TursoSinkReader {
    pub fn new(db_handle: DbHandle) -> Self {
        Self { db_handle }
    }
}

#[async_trait::async_trait]
impl SinkReader for TursoSinkReader {
    async fn read_blocks(&self) -> Result<HashMap<String, SnapshotBlock>> {
        let sql = format!(
            "SELECT b.id, b.parent_id, b.sort_key, b.content, b.content_type, b.source_language, \
             b.source_name, b.properties, b.property_kinds, b.marks, b.created_at, b.updated_at, \
             COALESCE((SELECT \
             json_group_array(tag) FROM block_tags WHERE block_id = b.id), '[]') AS tags, \
             COALESCE((SELECT json_group_array(required_id) FROM block_requires WHERE block_id = \
             b.id), '[]') AS requires, COALESCE((SELECT json_group_array(lesson_id) FROM \
             advice_suppressed WHERE anchor_id = b.id), '[]') AS advice_suppressed, \
             COALESCE((SELECT json_group_array(target_id) FROM block_contributes_to WHERE \
             block_id = b.id), '[]') AS contributes_to FROM {table} b \
             WHERE b.id != '{sentinel}'",
            table = BLOCK_WRITE_TABLE,
            sentinel = holon_api::EntityUri::no_parent().as_str(),
        );
        let rows = self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| anyhow::anyhow!("TursoSinkReader query failed: {e}"))?;
        let mut out = HashMap::with_capacity(rows.len());
        for row in rows {
            // sort_key is the SQL sink's internal ordering encoding — read it for
            // the diff base before `try_from` consumes the row; it is no longer a
            // field of the domain `Block` (ADR 0005).
            let sort_key = row
                .get("sort_key")
                .and_then(|v| v.as_string())
                .map(str::to_string)
                .unwrap_or_else(default_sort_key);
            let block = Block::try_from(row)
                .map_err(|e| anyhow::anyhow!("TursoSinkReader: Block::try_from row: {e}"))?;
            out.insert(block.id.to_string(), SnapshotBlock { block, sort_key });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The self-parented `sentinel:no_parent` FK-anchor row must never enter
    /// the projection diff base: an armed reseed would diff it against the
    /// Loro snapshot (which never holds it), emit
    /// `delete('sentinel:no_parent')`, and `prepare_delete` fails loud on
    /// the self-parent cycle — wedging the projector in a permanent
    /// error/reseed storm (2026-07-11 keystone seed 7a4d441d).
    #[tokio::test]
    async fn read_blocks_excludes_the_sentinel_fk_anchor() {
        let (_backend, db_handle) = crate::storage::turso::TursoBackend::new_in_memory()
            .await
            .expect("in-memory turso");
        db_handle
            .execute_ddl(
                "CREATE TABLE block_raw (
                    id TEXT PRIMARY KEY,
                    parent_id TEXT,
                    depth INTEGER NOT NULL DEFAULT 0,
                    sort_key TEXT NOT NULL DEFAULT 'A0',
                    content TEXT NOT NULL DEFAULT '',
                    content_type TEXT NOT NULL DEFAULT 'text',
                    source_language TEXT,
                    source_name TEXT,
                    properties TEXT,
                    property_kinds TEXT,
                    marks TEXT,
                    collapsed INTEGER NOT NULL DEFAULT 0,
                    widget_only INTEGER NOT NULL DEFAULT 0,
                    completed INTEGER NOT NULL DEFAULT 0,
                    block_type TEXT NOT NULL DEFAULT 'text',
                    created_at INTEGER NOT NULL DEFAULT 0,
                    updated_at INTEGER NOT NULL DEFAULT 0,
                    _change_origin TEXT
                )",
            )
            .await
            .expect("create block_raw");
        for ddl in [
            "CREATE TABLE block_tags (block_id TEXT, tag TEXT)",
            "CREATE TABLE block_requires (block_id TEXT, required_id TEXT)",
            "CREATE TABLE advice_suppressed (anchor_id TEXT, lesson_id TEXT)",
            "CREATE TABLE block_contributes_to (block_id TEXT, target_id TEXT)",
        ] {
            db_handle.execute_ddl(ddl).await.expect("create junction");
        }
        db_handle
            .execute(
                "INSERT INTO block_raw (id, parent_id) VALUES ('sentinel:no_parent', \
                 'sentinel:no_parent'), ('block:a', 'sentinel:no_parent')",
                vec![],
            )
            .await
            .expect("seed rows");

        let out = TursoSinkReader::new(db_handle)
            .read_blocks()
            .await
            .expect("read_blocks");
        assert!(
            out.contains_key("block:a"),
            "real blocks must be read: {:?}",
            out.keys().collect::<Vec<_>>()
        );
        assert!(
            !out.contains_key("sentinel:no_parent"),
            "the FK-anchor sentinel must be excluded from the diff base"
        );
    }
}
