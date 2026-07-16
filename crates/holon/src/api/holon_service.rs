//! Shared service layer for BackendEngine operations.
//!
//! Both the MCP server and test infrastructure delegate to `HolonService`
//! instead of calling `BackendEngine` directly.  This gives MCP code paths
//! test coverage through PBTs while keeping MCP-specific concerns
//! (tool descriptions, `CallToolResult` wrapping, transport) in the MCP crate.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::HistoryStore;
use holon_api::OperationDescriptor;
use holon_api::QueryContext;
use holon_api::QueryLanguage;
use holon_api::Value;
use holon_core::storage::types::StorageEntity;

use crate::api::backend_engine::BackendEngine;
use crate::storage::turso::RowChangeStream;

/// Result of a query execution, including timing.
pub struct QueryResult {
    pub rows: Vec<holon_api::StorageEntity>,
    pub duration: std::time::Duration,
}

/// A table/view/matview entry returned by `list_tables`.
#[derive(Debug, Clone)]
pub struct TableEntry {
    pub name: String,
    pub definition: Option<String>,
}

/// Schema listing returned by `list_tables`.
#[derive(Debug, Clone)]
pub struct SchemaListing {
    pub tables: Vec<TableEntry>,
    pub views: Vec<TableEntry>,
    pub materialized_views: Vec<TableEntry>,
}

/// Column definition for `create_table`.
pub struct ColumnDef {
    pub name: String,
    pub sql_type: String,
    pub primary_key: bool,
    pub default: Option<String>,
}

/// Shared service layer wrapping `BackendEngine`.
///
/// Provides the operations that both MCP tools and integration tests need:
/// query compilation+execution, operation dispatch, undo/redo, schema
/// introspection, etc.
pub struct HolonService {
    engine: Arc<BackendEngine>,
    /// The provenance origin stamped on operations dispatched through this
    /// facade. A human frontend/web-worker session uses [`OpOrigin::User`]; the
    /// embedded MCP server constructs the facade with [`OpOrigin::Agent`] so
    /// agent-driven ops carry their session/tool-call identity (C2a).
    origin: holon_api::OpOrigin,
}

impl HolonService {
    /// A human-session facade (UI / web-worker): ops are stamped
    /// [`OpOrigin::User`].
    pub fn new(engine: Arc<BackendEngine>) -> Self {
        Self::new_with_origin(engine, holon_api::OpOrigin::User)
    }

    /// A facade whose dispatched ops carry `origin` (e.g. the MCP server passes
    /// an [`OpOrigin::Agent`] carrying the driving agent's session/tool-call
    /// id).
    pub fn new_with_origin(engine: Arc<BackendEngine>, origin: holon_api::OpOrigin) -> Self {
        Self { engine, origin }
    }

    pub fn engine(&self) -> &Arc<BackendEngine> {
        &self.engine
    }

    // ── Query operations ──────────────────────────────────────────────

    /// Build a `QueryContext` from explicit context_id / context_parent_id.
    pub async fn build_context(
        &self,
        context_id: Option<&str>,
        context_parent_id: Option<&str>,
    ) -> Option<QueryContext> {
        let id = context_id?;
        let uri = EntityUri::parse(id).expect("context_id is not a valid EntityUri");
        let path = self
            .engine
            .blocks()
            .lookup_block_path(&uri)
            .await
            .unwrap_or_else(|_| format!("/{}", id));
        Some(QueryContext::for_block_with_path(
            &uri,
            context_parent_id
                .map(|s| EntityUri::parse(s).expect("context_parent_id is not a valid EntityUri")),
            path,
        ))
    }

    /// Compile a query (PRQL / GQL / SQL) to its final SQL form without
    /// executing.
    pub fn compile_query(&self, query: &str, language: QueryLanguage) -> Result<String> {
        self.engine
            .compile_to_sql(query, language)
            .context("Failed to compile query")
    }

    /// Compile and execute a query, returning rows and timing.
    pub async fn execute_query(
        &self,
        query: &str,
        language: QueryLanguage,
        params: HashMap<String, Value>,
        context: Option<QueryContext>,
    ) -> Result<QueryResult> {
        let sql = self.compile_query(query, language)?;
        self.execute_sql(sql, params, context).await
    }

    /// Execute pre-compiled SQL, returning rows and timing.
    pub async fn execute_sql(
        &self,
        sql: String,
        params: HashMap<String, Value>,
        context: Option<QueryContext>,
    ) -> Result<QueryResult> {
        let t0 = crate::util::MonotonicInstant::now();
        let rows = self
            .engine
            .execute_query(sql, params, context)
            .await
            .context("Failed to execute query")?;
        Ok(QueryResult {
            rows,
            duration: t0.elapsed(),
        })
    }

    /// Execute raw SQL directly against the database, bypassing query
    /// compilation.
    pub async fn execute_raw_sql(
        &self,
        sql: &str,
        params: HashMap<String, Value>,
    ) -> Result<QueryResult> {
        let t0 = crate::util::MonotonicInstant::now();
        let rows = self
            .engine
            .db_handle()
            .query(sql, params)
            .await
            .context("Failed to execute raw SQL")?;
        Ok(QueryResult {
            rows,
            duration: t0.elapsed(),
        })
    }

    // ── History / provenance (C2b, ADR 0024 P8) ───────────────────────

    /// Query the op/effect history relation (`block_history`) through the typed
    /// [`HistoryStore`] accessor. The relation is a disclosed ephemeral cache;
    /// raw SQL over `block_history` via [`Self::execute_raw_sql`] is equally
    /// sanctioned (Martin's ruling 2026-07-11) — this is the thin typed surface.
    pub async fn query_history(
        &self,
        filter: &holon_api::HistoryQuery,
    ) -> Result<Vec<holon_api::HistoryEvent>> {
        self.history_store().query(filter).await
    }

    /// Count events matching `filter` — the "postponed N times" primitive.
    pub async fn count_history(&self, filter: &holon_api::HistoryQuery) -> Result<u64> {
        self.history_store().count(filter).await
    }

    /// The typed history accessor over this engine's db handle. Fidelity matches
    /// the production (Loro-projected) wiring in [`crate::api::BackendEngine`];
    /// for reads it is disclosure only. The `block_history` table is boot-owned
    /// by `HistorySchemaModule`, so the accessor never lazily creates it.
    fn history_store(&self) -> crate::api::TursoHistoryStore {
        crate::api::TursoHistoryStore::new(
            self.engine.db_handle().clone(),
            holon_api::HistoryFidelity::Loro,
        )
    }

    /// Compile and start watching a query for CDC changes.
    /// Returns the initial-data stream.
    pub async fn query_and_watch(
        &self,
        query: &str,
        language: QueryLanguage,
        params: HashMap<String, Value>,
        context: Option<QueryContext>,
    ) -> Result<RowChangeStream> {
        let sql = self.compile_query(query, language)?;
        self.engine
            .query_and_watch(sql, params, context)
            .await
            .context("Failed to query and watch")
    }

    // ── Operation dispatch ────────────────────────────────────────────

    /// Execute an operation on an entity, returning the optional response
    /// value.
    pub async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<Option<Value>> {
        // HolonService is a session facade (human web-worker/UI, or the MCP
        // server acting for an agent); its configured `origin` states which.
        // Rule/sync/ingest ops do not route through it.
        self.engine
            .execute_operation(entity_name, op_name, params, self.origin.clone())
            .await
            .context(format!(
                "Failed to execute operation '{}' on entity '{}'",
                op_name, entity_name
            ))
    }

    /// List available operations for an entity.
    pub async fn available_operations(&self, entity_name: &str) -> Vec<OperationDescriptor> {
        self.engine.available_operations(entity_name).await
    }

    // ── Undo / Redo ───────────────────────────────────────────────────

    pub async fn undo(&self) -> Result<holon_api::UndoOutcome> {
        self.engine.undo().await
    }

    pub async fn redo(&self) -> Result<holon_api::UndoOutcome> {
        self.engine.redo().await
    }

    pub async fn can_undo(&self) -> bool {
        self.engine.can_undo().await
    }

    pub async fn can_redo(&self) -> bool {
        self.engine.can_redo().await
    }

    // ── Schema introspection ──────────────────────────────────────────

    /// List all tables, views, and materialized views.
    pub async fn list_tables(&self) -> Result<SchemaListing> {
        let sql = r#"
            SELECT name, type, sql AS definition
            FROM sqlite_master
            WHERE type IN ('table', 'view')
              AND name NOT LIKE 'sqlite_%'
              AND name NOT LIKE '_litestream_%'
            ORDER BY type, name
        "#;

        let rows = self
            .engine
            .db_handle()
            .query(sql, HashMap::new())
            .await
            .context("Failed to list tables")?;

        let mut tables = Vec::new();
        let mut views = Vec::new();

        for row in &rows {
            let name = match row.get("name") {
                Some(Value::String(s)) => s.clone(),
                _ => continue,
            };
            let obj_type = match row.get("type") {
                Some(Value::String(s)) => s.clone(),
                _ => continue,
            };
            let definition = match row.get("definition") {
                Some(Value::String(s)) => Some(s.clone()),
                _ => None,
            };

            let entry = TableEntry { name, definition };
            match obj_type.as_str() {
                "table" => tables.push(entry),
                "view" => views.push(entry),
                _ => {}
            }
        }

        let matview_rows = self
            .engine
            .db_handle()
            .query("PRAGMA materialized_views", HashMap::new())
            .await
            .context("querying PRAGMA materialized_views for schema listing")?;

        let materialized_views = matview_rows
            .iter()
            .filter_map(|row| {
                let name = match row.get("name") {
                    Some(Value::String(s)) => s.clone(),
                    _ => return None,
                };
                let definition = match row.get("sql") {
                    Some(Value::String(s)) => Some(s.clone()),
                    _ => None,
                };
                Some(TableEntry { name, definition })
            })
            .collect();

        Ok(SchemaListing {
            tables,
            views,
            materialized_views,
        })
    }

    // ── DDL helpers ───────────────────────────────────────────────────

    /// Create a table with the given columns.
    pub async fn create_table(&self, table_name: &str, columns: &[ColumnDef]) -> Result<()> {
        let column_defs: Vec<String> = columns
            .iter()
            .map(|col| {
                let mut def = format!("{} {}", col.name, col.sql_type);
                if col.primary_key {
                    def.push_str(" PRIMARY KEY");
                }
                if let Some(ref default) = col.default {
                    def.push_str(&format!(" DEFAULT {}", default));
                }
                def
            })
            .collect();

        let sql = format!(
            "CREATE TABLE IF NOT EXISTS {} ({})",
            table_name,
            column_defs.join(", ")
        );

        self.engine
            .execute_query(sql, HashMap::new(), None)
            .await
            .context(format!("Failed to create table '{}'", table_name))?;
        Ok(())
    }

    /// Insert rows into a table. Returns the number of rows inserted.
    pub async fn insert_data(
        &self,
        table_name: &str,
        rows: &[HashMap<String, Value>],
    ) -> Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }

        let columns: Vec<String> = rows[0].keys().cloned().collect();
        let placeholders: Vec<String> = (0..columns.len()).map(|i| format!("${}", i + 1)).collect();
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            table_name,
            columns.join(", "),
            placeholders.join(", ")
        );

        for (row_idx, row) in rows.iter().enumerate() {
            let mut values = HashMap::new();
            for (i, col) in columns.iter().enumerate() {
                if let Some(val) = row.get(col) {
                    values.insert(format!("{}", i + 1), val.clone());
                }
            }
            self.engine
                .execute_query(sql.clone(), values, None)
                .await
                .context(format!(
                    "Failed to insert into '{}' at row {}",
                    table_name, row_idx
                ))?;
        }

        Ok(rows.len())
    }

    /// Drop a table.
    pub async fn drop_table(&self, table_name: &str) -> Result<()> {
        let sql = format!("DROP TABLE IF EXISTS {}", table_name);
        self.engine
            .execute_query(sql, HashMap::new(), None)
            .await
            .context(format!("Failed to drop table '{}'", table_name))?;
        Ok(())
    }

    // ── Domain operations ─────────────────────────────────────────────

    /// Rank active tasks using WSJF.
    pub async fn rank_tasks(&self) -> Result<crate::petri::RankResult> {
        self.engine
            .blocks()
            .rank_tasks()
            .await
            .context("Failed to rank tasks")
    }
}
