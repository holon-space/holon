//! The SUT component of the SQL-storage slice: a [`CapProvider`] wrapping a
//! **real** Turso `BackendEngine` (the production storage + IVM matview layer),
//! driven through the production block operations — but with **none** of the
//! reactive engine, frontend, navigation, or CDC-mirror machinery that the
//! monolithic `E2ESut` carries.
//!
//! This is deliberately the storage-layer slice of the *future `E2ESut`
//! replacement*: it reuses `E2ESut`'s own SQL realization (the same `block_raw`
//! queries, the shared [`parse_block_row`](crate::pbt::sut_row_parsing)) so that
//! when the monolith is retired (the §F2 convergence) the composed components
//! already cover its storage surface.
//!
//! It provides two caps:
//! - [`SutBackend`] over `block_raw` — so **every already-wired block-tree
//!   invariant** (`no_parent_cycles`, `source_language`, `blocks_match`,
//!   `no_orphan`, `block_content`, `block_parent`) runs over the Turso
//!   realization *for free* (the §6 payoff — a third storage backing the same
//!   catalog, no catalog change).
//! - [`SutSqlProjection`] — the SQL read surface. The base-table family
//!   (`block_raw`/`block_tags`/`block` matview) is realized over real Turso SQL;
//!   the navigation/focus/watch family is honest-empty (§5.1): this slice has no
//!   reactive engine driving navigation or CDC watches, so an empty projection
//!   is the honest answer, never a fabricated row.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use holon::api::BackendEngine;
use holon_api::{Block, ContentType, EntityUri, SourceLanguage, StorageEntity, Value};
use holon_pbt_core::capabilities::{SutBackend, SutBlockTreeWrite, SutSqlProjection};
use holon_pbt_core::composition::{CapMap, CapProvider};

use crate::pbt::sut_row_parsing::parse_block_rows;

/// A composition component wrapping a real Turso `BackendEngine`. Mutations go
/// through the production block operation dispatcher; reads go through raw SQL
/// against the same connection — exactly the data path `E2ESut` uses, minus the
/// app.
pub struct SqlProjectionComponent {
    engine: Arc<BackendEngine>,
}

impl SqlProjectionComponent {
    pub fn new(engine: Arc<BackendEngine>) -> Self {
        Self { engine }
    }

    /// Run a read-only SQL statement and return the raw rows.
    async fn query(&self, sql: &str) -> Vec<StorageEntity> {
        self.engine
            .db_handle()
            .query(sql, HashMap::new())
            .await
            .unwrap_or_else(|e| panic!("SqlProjectionComponent query failed ({sql}): {e}"))
    }

    /// Create a block with an explicit id through the production `block`/`create`
    /// operation. Returns the supplied id so callers can build their reference
    /// from exactly the id the store holds.
    pub async fn create_block(
        &self,
        id: &EntityUri,
        parent: &EntityUri,
        content: &str,
    ) -> EntityUri {
        let mut params: StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String(id.to_string()));
        params.insert("parent_id".into(), Value::String(parent.to_string()));
        params.insert("content".into(), Value::String(content.to_string()));
        params.insert("content_type".into(), ContentType::Text.into());
        self.execute_op("create", params).await;
        id.clone()
    }

    /// Create a source block (code block) with a language through the production
    /// `block`/`create` operation — exercises `inv-source-language-iff-source`.
    pub async fn create_source_block(
        &self,
        id: &EntityUri,
        parent: &EntityUri,
        language: &str,
        content: &str,
    ) -> EntityUri {
        let lang: SourceLanguage = language
            .parse()
            .unwrap_or_else(|_| panic!("unknown source language {language:?}"));
        let mut params: StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String(id.to_string()));
        params.insert("parent_id".into(), Value::String(parent.to_string()));
        params.insert("content".into(), Value::String(content.to_string()));
        params.insert("content_type".into(), ContentType::Source.into());
        params.insert("source_language".into(), lang.into());
        self.execute_op("create", params).await;
        id.clone()
    }

    /// Update a block's `content` column via the production `set_field` operation.
    pub async fn update_content(&self, id: &EntityUri, content: &str) {
        let mut params: StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String(id.to_string()));
        params.insert("field".into(), Value::String("content".to_string()));
        params.insert("value".into(), Value::String(content.to_string()));
        self.execute_op("set_field", params).await;
    }

    /// Set a block's `task_state` property via the production `set_field`
    /// operation — `set_field` routes a non-column field into the `properties`
    /// JSON (`json_set(properties,'$.task_state',…)`), the same scalar
    /// [`SutSqlProjection::block_task_state`] reads back. Used to seed the
    /// combined SQL+Loro slice's `inv-task-state-storage-coherence`.
    pub async fn set_task_state(&self, id: &EntityUri, state: &str) {
        let mut params: StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String(id.to_string()));
        params.insert("field".into(), Value::String("task_state".to_string()));
        params.insert("value".into(), Value::String(state.to_string()));
        self.execute_op("set_field", params).await;
    }

    /// Delete a block via the production `delete` operation.
    pub async fn delete_block(&self, id: &EntityUri) {
        let mut params: StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String(id.to_string()));
        self.execute_op("delete", params).await;
    }

    async fn execute_op(&self, op: &str, params: StorageEntity) {
        let entity = "block".to_string().into();
        self.engine
            .execute_operation(&entity, op, params)
            .await
            .unwrap_or_else(|e| panic!("block/{op} operation failed: {e}"));
    }

    async fn blocks_from_block_raw(&self) -> Vec<Block> {
        let rows = self
            .query(crate::pbt::sut_row_parsing::BLOCK_RAW_SNAPSHOT_SQL)
            .await;
        parse_block_rows(&rows)
    }

    fn cell(row: &StorageEntity, col: &str) -> Option<String> {
        row.get(col).and_then(|v| v.as_string()).map(str::to_string)
    }
}

#[async_trait::async_trait(?Send)]
impl SutBackend for SqlProjectionComponent {
    /// No CDC matview mirror in this lean slice, so the "live" view *is* the
    /// `block_raw` base table (the same parse path `E2ESut` uses for its
    /// `/block_raw` store).
    async fn live_block_snapshot(&self) -> Vec<Block> {
        self.blocks_from_block_raw().await
    }

    async fn block_raw_snapshot(&self) -> Vec<Block> {
        self.blocks_from_block_raw().await
    }

    /// No navigation/focus projection in this slice (no reactive engine). No
    /// cap-selected invariant reads it; an empty mirror is the honest answer.
    async fn live_focus_root_rows(&self) -> Vec<(String, String)> {
        Vec::new()
    }
}

#[async_trait::async_trait(?Send)]
impl SutSqlProjection for SqlProjectionComponent {
    async fn block_row(&self, id: &EntityUri) -> Option<Vec<String>> {
        let escaped = id.as_str().replace('\'', "''");
        let rows = self
            .query(&format!("SELECT * FROM block WHERE id = '{escaped}'"))
            .await;
        rows.into_iter().next().map(|row| {
            let mut fields: Vec<String> = row
                .into_values()
                .map(|v| v.as_string().unwrap_or_default().to_string())
                .collect();
            fields.sort();
            fields
        })
    }

    async fn all_block_ids(&self) -> BTreeSet<EntityUri> {
        self.query("SELECT id FROM block_raw")
            .await
            .iter()
            .filter_map(|r| {
                Self::cell(r, "id").map(|s| {
                    EntityUri::parse(&s).expect("block id from SQL must be a valid EntityUri")
                })
            })
            .collect()
    }

    async fn sorted_children(&self, parent: &EntityUri) -> Vec<EntityUri> {
        let escaped = parent.as_str().replace('\'', "''");
        self.query(&format!(
            "SELECT id FROM block_raw WHERE parent_id = '{escaped}' ORDER BY sort_key, id"
        ))
        .await
        .iter()
        .filter_map(|r| {
            Self::cell(r, "id")
                .map(|s| EntityUri::parse(&s).expect("block id from SQL must be a valid EntityUri"))
        })
        .collect()
    }

    /// No registered CDC watches in this slice — there is no reactive engine to
    /// drive them. Honest `None` (the query id is never registered).
    async fn watch_row_count(&self, _: &str) -> Option<usize> {
        None
    }

    async fn block_raw_row(&self, id: &EntityUri) -> Option<Vec<String>> {
        let escaped = id.as_str().replace('\'', "''");
        let rows = self
            .query(&format!("SELECT * FROM block_raw WHERE id = '{escaped}'"))
            .await;
        rows.into_iter().next().map(|row| {
            let mut fields: Vec<String> = row
                .into_values()
                .map(|v| v.as_string().unwrap_or_default().to_string())
                .collect();
            fields.sort();
            fields
        })
    }

    async fn block_tag_block_ids(&self) -> BTreeSet<EntityUri> {
        self.query("SELECT DISTINCT block_id FROM block_tags")
            .await
            .iter()
            .filter_map(|r| {
                Self::cell(r, "block_id").map(|s| {
                    EntityUri::parse(&s).expect("block_tags.block_id must be a valid EntityUri")
                })
            })
            .collect()
    }

    async fn block_task_state(&self, id: &EntityUri) -> Option<String> {
        let escaped = id.as_str().replace('\'', "''");
        let rows = self
            .query(&format!(
                "SELECT json_extract(properties, '$.task_state') AS task_state \
                 FROM block_raw WHERE id = '{escaped}'"
            ))
            .await;
        rows.into_iter()
            .next()
            .and_then(|r| Self::cell(&r, "task_state"))
    }

    async fn block_content(&self, id: &EntityUri) -> Option<String> {
        let escaped = id.as_str().replace('\'', "''");
        let rows = self
            .query(&format!(
                "SELECT content FROM block_raw WHERE id = '{escaped}'"
            ))
            .await;
        rows.into_iter()
            .next()
            .and_then(|r| Self::cell(&r, "content"))
    }
}
// No `SutFocusProjection` impl: this slice drives no navigation, so the
// focus/nav invariants (`inv-navigation-focus`/`inv-focus-roots`) DESELECT here
// honestly (C-5, 2026-07-02) — no honest-empty focus family to pass vacuously.

impl CapProvider for SqlProjectionComponent {
    fn register(self: Arc<Self>, caps: &mut CapMap) {
        caps.insert(self.clone() as Arc<dyn SutBackend>);
        caps.insert(self.clone() as Arc<dyn SutSqlProjection>);
        // Structural block-tree writes are NOT hand-rolled per component: they are
        // the production `block` operations, so we register the reusable
        // `OpDispatchWriter` over this component's `BackendEngine` (see
        // `pbt::op_write_cap`). Any storage component holding an engine reuses the
        // SAME `SutBlockTreeWrite` impl — no per-component forwarding boilerplate.
        caps.insert(Arc::new(crate::pbt::op_write_cap::OpDispatchWriter::new(
            self.engine.clone(),
        )) as Arc<dyn SutBlockTreeWrite>);
    }
}
