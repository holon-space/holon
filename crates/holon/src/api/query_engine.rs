//! Turso implementation of the query capability, plus the Turso-private
//! raw-SQL extension trait.
//!
//! The storage-agnostic core trait lives in [`holon_api::query_engine`]
//! (storage de-leak Stage 10); this module implements it for the concrete
//! Turso [`BackendEngine`] and adds [`SqlQueryEngine`] for the callers that
//! legitimately speak SQL: MCP debug tools, integration tests, and
//! holon-internal code. holon-frontend must never see this extension trait.

use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;
use holon_api::EnrichedChangeStream;
use holon_api::EntityUri;
use holon_api::LinkCandidate;
use holon_api::QueryContext;
use holon_api::QueryLanguage;
use holon_api::Value;
pub use holon_api::query_engine::QueryEngine;

use crate::api::BackendEngine;
use crate::storage::turso::RowChangeStream;

/// Raw-SQL extension of [`QueryEngine`] — Turso-typed (`RowChangeStream`) and
/// SQL-string-typed. Implemented by [`BackendEngine`] only; deliberately NOT
/// part of holon-api so the storage-agnostic layers cannot reach it.
#[async_trait]
pub trait SqlQueryEngine: QueryEngine {
    /// Compile a query in any supported language (PRQL/GQL/SQL) to final SQL.
    fn compile_to_sql(&self, query: &str, language: QueryLanguage) -> Result<String>;

    /// Execute a SQL query, set up CDC streaming, and return a stream whose
    /// first batch is the initial results, followed by CDC deltas.
    async fn query_and_watch(
        &self,
        sql: String,
        params: HashMap<String, Value>,
        context: Option<QueryContext>,
    ) -> Result<RowChangeStream>;

    /// Execute a SQL query once and return all rows.
    async fn execute_query(
        &self,
        sql: String,
        params: HashMap<String, Value>,
        context: Option<QueryContext>,
    ) -> Result<Vec<holon_api::StorageEntity>>;
}

#[async_trait]
impl QueryEngine for BackendEngine {
    async fn lookup_block_path(&self, block_id: &EntityUri) -> Result<String> {
        self.blocks().lookup_block_path(block_id).await
    }

    async fn watch_query(
        &self,
        query: &str,
        language: QueryLanguage,
        params: HashMap<String, Value>,
        context: Option<QueryContext>,
    ) -> Result<EnrichedChangeStream> {
        let sql = BackendEngine::compile_to_sql(self, query, language)?;
        let raw = BackendEngine::query_and_watch(self, sql, params, context).await?;
        Ok(crate::api::ui_watcher::enrich_stream(
            raw,
            self.profile_resolver().clone(),
        ))
    }

    async fn search_link_candidates(&self, filter: &str) -> Result<Vec<LinkCandidate>> {
        use crate::storage::BLOCK_READ_TABLE;
        let escaped = filter.replace('\'', "''");
        // Subquery wrapping required — Turso rejects bare UNION.
        // Page rows: block has a 'Page' tag in block_tags junction table;
        // surface the first content line as the label.
        let sql = format!(
            "SELECT * FROM (SELECT id, content AS label FROM {BLOCK_READ_TABLE} WHERE content \
             LIKE '%{escaped}%' LIMIT 15) UNION ALL SELECT * FROM (SELECT b.id, substr(b.content, \
             1, instr(b.content || char(10), char(10)) - 1) AS label FROM {BLOCK_READ_TABLE} b \
             JOIN block_tags bt ON bt.block_id = b.id WHERE bt.tag = 'Page' AND b.content LIKE \
             '%{escaped}%' LIMIT 5)"
        );
        let rows = BackendEngine::execute_query(self, sql, HashMap::new(), None).await?;
        rows.into_iter()
            .map(|row| {
                let raw_id = row
                    .get("id")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| anyhow::anyhow!("link-search row missing 'id': {row:?}"))?
                    .to_string();
                let id = EntityUri::parse(&raw_id).map_err(|e| {
                    anyhow::anyhow!("link-search row id {raw_id:?} is not a valid EntityUri: {e}")
                })?;
                let label = row
                    .get("label")
                    .and_then(|v| v.as_string())
                    .unwrap_or("(untitled)")
                    .to_string();
                Ok(LinkCandidate { id, label })
            })
            .collect()
    }

    async fn block_content_by_id(&self, id: &EntityUri) -> Result<Option<String>> {
        use crate::storage::BLOCK_WRITE_TABLE;
        let escaped = id.to_string().replace('\'', "''");
        let sql = format!("SELECT content FROM {BLOCK_WRITE_TABLE} WHERE id = '{escaped}'");
        let rows = BackendEngine::execute_query(self, sql, HashMap::new(), None).await?;
        Ok(rows.into_iter().next().and_then(|r| {
            r.get("content")
                .and_then(|v| v.as_string())
                .map(str::to_string)
        }))
    }
}

#[async_trait]
impl SqlQueryEngine for BackendEngine {
    fn compile_to_sql(&self, query: &str, language: QueryLanguage) -> Result<String> {
        BackendEngine::compile_to_sql(self, query, language)
    }

    async fn query_and_watch(
        &self,
        sql: String,
        params: HashMap<String, Value>,
        context: Option<QueryContext>,
    ) -> Result<RowChangeStream> {
        BackendEngine::query_and_watch(self, sql, params, context).await
    }

    async fn execute_query(
        &self,
        sql: String,
        params: HashMap<String, Value>,
        context: Option<QueryContext>,
    ) -> Result<Vec<holon_api::StorageEntity>> {
        BackendEngine::execute_query(self, sql, params, context).await
    }
}
