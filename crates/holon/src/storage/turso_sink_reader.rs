//! Production [`SinkReader`] over the Turso SQL sink.
//!
//! Lives in the storage layer (Phase 2.5, C4 of the architecture plan): the
//! consolidation layer (`sync/loro_sync_controller.rs`) defines the
//! [`SinkReader`] seam and must not import concrete Turso types; this concrete
//! reader is constructed at wiring time (`sync/loro_module.rs`).

use std::collections::HashMap;

use anyhow::Result;
use holon_api::block::Block;
use holon_core::fractional_index::default_sort_key;

use crate::api::SnapshotBlock;
use crate::storage::BLOCK_WRITE_TABLE;
use crate::storage::turso::DbHandle;
use crate::sync::SinkReader;

/// Production [`SinkReader`]: reads the `block_raw` base table directly — NOT the
/// `block` matview, which can lag `block_raw` under IVM and would make the
/// projection see stale state and re-emit redundant writes. `tags`/`requires`
/// are hydrated from their junction tables; both are part of the block
/// equivalence relation (`blocks_differ` iterates `EdgeField::ALL`).
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
            "SELECT b.id, b.parent_id, b.sort_key, b.content, b.content_type, \
                    b.source_language, b.source_name, b.properties, b.marks, \
                    b.created_at, b.updated_at, \
                    COALESCE((SELECT json_group_array(tag) FROM block_tags \
                              WHERE block_id = b.id), '[]') AS tags, \
                    COALESCE((SELECT json_group_array(required_id) FROM block_requires \
                              WHERE block_id = b.id), '[]') AS requires, \
                    COALESCE((SELECT json_group_array(lesson_id) FROM advice_suppressed \
                              WHERE anchor_id = b.id), '[]') AS advice_suppressed \
             FROM {table} b",
            table = BLOCK_WRITE_TABLE,
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
