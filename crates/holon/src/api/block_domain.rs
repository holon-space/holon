use anyhow::Result;
use std::collections::HashMap;

use super::backend_engine::{BackendEngine, QueryContext};
use holon_api::{ActionSpec, EntityUri, QueryLanguage, Value, WidgetSpec, uri_from_row};

use crate::storage::turso::RowChangeStream;

const GRAPH_EAV_SCHEMA: &str = include_str!("../../sql/schema/graph_eav.sql");

const BLOCK_PATH_LOOKUP_SQL: &str = include_str!("../../sql/queries/block_path_lookup.sql");
const TASK_BLOCKS_FOR_PETRI_SQL: &str = include_str!("../../sql/queries/task_blocks_for_petri.sql");
const BLOCK_WITH_QUERY_SOURCE_SQL: &str =
    include_str!("../../sql/queries/block_with_query_source.sql");

pub use holon_api::ROOT_LAYOUT_BLOCK_ID;

/// Domain layer for block-specific operations.
///
/// Wraps a `BackendEngine` reference and provides methods that encode
/// domain knowledge about blocks: layout discovery, rendering, task ranking,
/// and database initialization. The underlying `BackendEngine` remains a
/// reusable, domain-agnostic query engine.
pub struct BlockDomain<'a> {
    engine: &'a BackendEngine,
}

impl<'a> BlockDomain<'a> {
    pub(crate) fn new(engine: &'a BackendEngine) -> Self {
        Self { engine }
    }

    /// Look up a block's path from the block_with_path materialized view.
    #[tracing::instrument(skip(self))]
    pub async fn lookup_block_path(&self, block_id: &EntityUri) -> Result<String> {
        let mut params = HashMap::new();
        params.insert("block_id".to_string(), Value::String(block_id.to_string()));

        let rows = self
            .engine
            .execute_query(BLOCK_PATH_LOOKUP_SQL.to_string(), params, None)
            .await?;

        if let Some(row) = rows.first() {
            if let Some(Value::String(path)) = row.get("path") {
                return Ok(path.clone());
            }
        }

        // Block not in block_with_path yet - use block_id as fallback path
        Ok(format!("/{}", block_id))
    }

    /// Render a block by its ID.
    ///
    /// Given a block ID, finds its query source child, compiles and executes the query,
    /// parses any render sibling into a RenderExpr, and returns a WidgetSpec + CDC stream.
    ///
    /// For the root block, global actions are added to the WidgetSpec.
    #[tracing::instrument(skip(self), fields(block_id = %block_id, is_root))]
    pub async fn render_block(
        &self,
        block_id: &EntityUri,
        preferred_variant: &Option<String>,
        is_root: bool,
    ) -> Result<(WidgetSpec, RowChangeStream)> {
        let block_info = match self.load_block_with_query_source(block_id).await {
            Ok(info) => info,
            Err(_e) if !is_root => {
                // Non-root block has no query source child — render as a leaf text block.
                // Root blocks must always have a query source (from index.org).
                return self.render_leaf_block(block_id).await;
            }
            Err(e) => return Err(e),
        };

        let query_source = block_info
            .get("query_source")
            .and_then(|v| v.as_string())
            .ok_or_else(|| anyhow::anyhow!("Block '{block_id}' has no query source child"))?
            .to_string();

        let query_language: QueryLanguage = block_info
            .get("query_language")
            .and_then(|v| v.as_string())
            .map(|s| s.parse::<QueryLanguage>())
            .transpose()
            .map_err(|e| anyhow::anyhow!("Block '{block_id}' has invalid query_language: {e}"))?
            .unwrap_or(QueryLanguage::HolonPrql);

        let parent_id = uri_from_row(&block_info, "parent_id").ok();

        let block_path = self.lookup_block_path(block_id).await?;

        let context = QueryContext::for_block_with_path(block_id, parent_id, block_path);

        let sql = self.engine.compile_to_sql(&query_source, query_language)?;

        let (mut widget_spec, change_stream) = self
            .engine
            .query_and_watch(sql, HashMap::new(), Some(context))
            .await?;

        widget_spec.render_expr = Self::parse_render_source(&block_info);

        if is_root {
            widget_spec.actions = Self::build_global_actions();
        }

        Ok((widget_spec, change_stream))
    }

    /// Render a leaf block (no query source child) via the `render_block()` widget.
    ///
    /// Returns the block's own data as a single-row WidgetSpec with a
    /// `render_block()` render expression. The shadow interpreter will then
    /// dispatch through profile resolution, creating EditableText nodes with
    /// proper operations for editable blocks.
    async fn render_leaf_block(
        &self,
        block_id: &EntityUri,
    ) -> Result<(WidgetSpec, RowChangeStream)> {
        let sql = "SELECT id, content, content_type, source_language, parent_id FROM block WHERE id = $block_id";
        let mut params = HashMap::new();
        params.insert("block_id".to_string(), Value::String(block_id.to_string()));
        let data = self
            .engine
            .execute_query(sql.to_string(), params, None)
            .await?;

        let render_expr = holon_api::render_types::RenderExpr::FunctionCall {
            name: "render_block".to_string(),
            args: Vec::new(),
        };

        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(tx);
        let change_stream = tokio_stream::wrappers::ReceiverStream::new(rx);

        Ok((
            WidgetSpec {
                render_expr,
                data,
                actions: vec![],
            },
            change_stream,
        ))
    }

    /// Load a block by ID and find its query source child + optional render sibling.
    #[tracing::instrument(skip(self))]
    async fn load_block_with_query_source(
        &self,
        block_id: &EntityUri,
    ) -> Result<HashMap<String, Value>> {
        let query_langs = QueryLanguage::sql_in_list();
        let sql = BLOCK_WITH_QUERY_SOURCE_SQL.replace("{query_langs}", &query_langs);

        let mut params = HashMap::new();
        params.insert("block_id".to_string(), Value::String(block_id.to_string()));

        let rows = self.engine.execute_query(sql, params, None).await?;

        if rows.is_empty() {
            anyhow::bail!(
                "Block '{}' not found or has no query source child (prql/gql/sql)",
                block_id
            );
        }

        Ok(rows[0].clone())
    }

    /// Parse a render_source into a RenderExpr.
    fn parse_render_source(
        block_info: &HashMap<String, Value>,
    ) -> holon_api::render_types::RenderExpr {
        if let Some(Value::String(source)) = block_info.get("render_source") {
            match crate::render_dsl::parse_render_dsl(source) {
                Ok(expr) => return expr,
                Err(e) => {
                    tracing::warn!("Failed to parse render_source, defaulting to table(): {e}");
                }
            }
        }

        holon_api::render_types::RenderExpr::FunctionCall {
            name: "table".to_string(),
            args: Vec::new(),
        }
    }

    fn build_global_actions() -> Vec<ActionSpec> {
        vec![
            ActionSpec::new("sync", "Sync", "*", "sync_from_remote").with_icon("sync"),
            ActionSpec::new("undo", "Undo", "*", "undo").with_icon("undo"),
            ActionSpec::new("redo", "Redo", "*", "redo").with_icon("redo"),
        ]
    }

    /// Initialize database schema and sample data if the database doesn't exist.
    pub async fn initialize_database_if_needed(&self, db_path: &std::path::PathBuf) -> Result<()> {
        // Initialize graph schema (idempotent — all IF NOT EXISTS)
        for stmt in crate::storage::sql_statements(GRAPH_EAV_SCHEMA) {
            self.engine
                .db_handle()
                .execute_ddl(stmt)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to execute graph schema DDL: {}", e))?;
        }

        let db_exists = db_path.exists();

        if !db_exists {
            let create_table_sql = r#"
                CREATE TABLE IF NOT EXISTS block (
                    id TEXT PRIMARY KEY,
                    parent_id TEXT,
                    depth INTEGER NOT NULL DEFAULT 0,
                    sort_key TEXT NOT NULL,
                    content TEXT NOT NULL,
                    collapsed INTEGER NOT NULL DEFAULT 0,
                    completed INTEGER NOT NULL DEFAULT 0,
                    block_type TEXT NOT NULL DEFAULT 'text',
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
                )
            "#;

            self.engine
                .execute_query(create_table_sql.to_string(), HashMap::new(), None)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to create blocks table: {}", e))?;

            use crate::storage::gen_key_between;

            let root_1_key = gen_key_between(None, None)
                .map_err(|e| anyhow::anyhow!("Failed to generate root-1 key: {}", e))?;
            let root_2_key = gen_key_between(Some(&root_1_key), None)
                .map_err(|e| anyhow::anyhow!("Failed to generate root-2 key: {}", e))?;

            let child_1_key = gen_key_between(None, None)
                .map_err(|e| anyhow::anyhow!("Failed to generate child-1 key: {}", e))?;
            let child_2_key = gen_key_between(Some(&child_1_key), None)
                .map_err(|e| anyhow::anyhow!("Failed to generate child-2 key: {}", e))?;

            let grandchild_1_key = gen_key_between(None, None)
                .map_err(|e| anyhow::anyhow!("Failed to generate grandchild-1 key: {}", e))?;

            let sample_data_sql = format!(
                r#"
                INSERT OR IGNORE INTO block (id, parent_id, depth, sort_key, content, block_type, completed)
                VALUES
                    ('root-1', NULL, 0, '{}', 'Welcome to Block Outliner', 'heading', 0),
                    ('child-1', 'root-1', 1, '{}', 'This is a child block', 'text', 0),
                    ('child-2', 'root-1', 1, '{}', 'Another child block', 'text', 1),
                    ('grandchild-1', 'child-1', 2, '{}', 'A nested grandchild', 'text', 0),
                    ('root-2', NULL, 0, '{}', 'Second top-level block', 'heading', 0)
            "#,
                root_1_key, child_1_key, child_2_key, grandchild_1_key, root_2_key
            );

            self.engine
                .execute_query(sample_data_sql.to_string(), HashMap::new(), None)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to insert sample data: {}", e))?;
        }

        Ok(())
    }

    /// Rank all active task blocks using WSJF (Weighted Shortest Job First).
    pub async fn rank_tasks(&self) -> Result<crate::petri::RankResult> {
        let rows = self
            .engine
            .execute_query(TASK_BLOCKS_FOR_PETRI_SQL.to_string(), HashMap::new(), None)
            .await?;

        let blocks: Vec<holon_api::block::Block> = rows
            .into_iter()
            .filter_map(|row| holon_api::Block::try_from(row).ok())
            .collect();

        Ok(crate::petri::rank_tasks(&blocks))
    }
}
