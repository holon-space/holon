use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

use tokio::sync::broadcast;

use crate::api::operation_dispatcher::OperationDispatcher;
use crate::core::datasource::OperationProvider;
use crate::storage::sql_utils::value_to_sql_literal;
use crate::storage::turso::{RowChange, RowChangeStream, TursoBackend};
use crate::storage::types::StorageEntity;
use crate::storage::{DbHandle, SqlTransformer};
use holon_api::{
    BatchWithMetadata, EntityUri, Operation, OperationDescriptor, QueryLanguage, Value, WidgetSpec,
};
use holon_core::{UndoAction, UndoStack};

/// Context for query compilation - determines what virtual tables resolve to
#[derive(Debug, Clone)]
pub struct QueryContext {
    /// Current block ID for `from children` resolution. None = root level (parent_id IS NULL)
    pub current_block_id: Option<EntityUri>,
    /// Parent of current block for `from siblings` resolution
    pub context_parent_id: Option<EntityUri>,
    /// Path prefix for descendants queries (e.g., "/block-123/%")
    /// Computed from block_with_path matview when context is created with path lookup.
    /// This is a SQL LIKE prefix, not an entity ID.
    pub context_path_prefix: Option<String>,
    /// Profile context for EntityProfile variant selection
    pub profile_context: Option<crate::entity_profile::ProfileContext>,
}

impl QueryContext {
    /// Create a root-level context (for queries at the top level)
    pub fn root() -> Self {
        Self {
            current_block_id: None,
            context_parent_id: None,
            context_path_prefix: None,
            profile_context: None,
        }
    }

    /// Create a context for a specific block
    pub fn for_block(block_id: EntityUri, parent_id: Option<EntityUri>) -> Self {
        Self {
            current_block_id: Some(block_id),
            context_parent_id: parent_id,
            context_path_prefix: None,
            profile_context: None,
        }
    }

    /// Create a context for a specific block with path prefix for descendants queries
    pub fn for_block_with_path(
        block_id: EntityUri,
        parent_id: Option<EntityUri>,
        path: String,
    ) -> Self {
        Self {
            current_block_id: Some(block_id),
            context_parent_id: parent_id,
            context_path_prefix: Some(format!("{}/", path)),
            profile_context: None,
        }
    }
}

/// PRQL stdlib defining virtual tables for hierarchical queries
///
/// Note: When $context_id is NULL, PRQL generates `parent_id = NULL` which is always false in SQL.
/// The `children` virtual table should be used with QueryContext::for_block() which sets a non-NULL context_id.
///
/// The `roots` virtual table returns top-level blocks (blocks whose parent is a document, not another block).
/// In the Holon data model, document-level blocks have parent_id starting with "doc:".
///
/// The `descendants` virtual table uses `block_with_path` materialized
/// view with path prefix matching. This enables efficient tree traversal using precomputed
/// hierarchical paths.
///
/// Note: We use `block_with_path` for descendants rather than PRQL's `loop` because
/// `let descendants = (... loop ...)` creates nested CTEs (outer CTE for `let`, inner
/// recursive CTE for `loop`) which prqlc doesn't flatten. The path-prefix approach is
/// also more efficient since `block_with_path` is a pre-existing materialized view.
const PRQL_STDLIB: &str = include_str!("../../sql/prql_stdlib.prql");

use crate::api::block_domain::BlockDomain;

/// Main render engine managing database, query compilation, and operations
pub struct BackendEngine {
    /// Handle for all database operations (query, execute, DDL, dependency tracking)
    db_handle: DbHandle,
    /// CDC broadcast sender for subscribing to change events
    cdc_broadcast: broadcast::Sender<BatchWithMetadata<RowChange>>,
    /// Operation dispatcher for routing operations
    dispatcher: Arc<OperationDispatcher>,
    /// Maps table names to entity names
    table_to_entity_map: Arc<RwLock<HashMap<String, String>>>,
    /// Undo/redo history
    undo_stack: Arc<RwLock<UndoStack>>,
    /// Manages materialized view lifecycle (creation, CDC, querying).
    matview_manager: crate::sync::MatviewManager,
    /// Entity profile resolver for per-row render + operation resolution
    profile_resolver: Arc<dyn crate::entity_profile::ProfileResolving>,
    /// SQL-level transformers applied after compilation (entity_name, _change_origin, json_agg)
    sql_transformers: Vec<Box<dyn SqlTransformer>>,
    /// Keeps the TursoBackend alive for as long as BackendEngine exists.
    ///
    /// The database actor is spawned by TursoBackend::new() and runs independently.
    /// The actor only exits when ALL senders to its channel are dropped. While DbHandle
    /// holds one sender clone, we also keep the TursoBackend (which holds another sender)
    /// alive to ensure the actor survives even if the DI container is dropped.
    ///
    /// This prevents the "Actor channel closed" bug where the actor dies between
    /// init_render_engine() completing (DI container dropped) and initial_widget() being called.
    _backend_keepalive: Arc<RwLock<TursoBackend>>,
}

impl BackendEngine {
    /// Create BackendEngine from dependencies (for dependency injection)
    ///
    /// This constructor takes a DbHandle (for all database operations including
    /// dependency-aware DDL), a CDC broadcast sender (for change notifications),
    /// and a reference to the TursoBackend to keep it alive.
    ///
    /// # Actor Lifetime Guarantee
    ///
    /// The `backend` parameter ensures the database actor stays alive for as long
    /// as this BackendEngine exists. The actor is spawned by `TursoBackend::new()`
    /// and only exits when ALL senders to its channel are dropped. By holding both
    /// the `db_handle` (one sender clone) AND the `backend` (another sender), we
    /// ensure the actor survives regardless of DI container lifetime.
    pub fn new(
        db_handle: DbHandle,
        cdc_broadcast: broadcast::Sender<BatchWithMetadata<RowChange>>,
        dispatcher: Arc<OperationDispatcher>,
        profile_resolver: Arc<dyn crate::entity_profile::ProfileResolving>,
        backend: Arc<RwLock<TursoBackend>>,
        sql_transformers: Vec<Box<dyn SqlTransformer>>,
    ) -> Result<Self> {
        let ddl_mutex = Arc::new(tokio::sync::Mutex::new(()));
        let matview_manager =
            crate::sync::MatviewManager::new(db_handle.clone(), cdc_broadcast.clone(), ddl_mutex);
        Ok(Self {
            db_handle,
            cdc_broadcast,
            dispatcher,
            table_to_entity_map: Arc::new(RwLock::new(HashMap::new())),
            undo_stack: Arc::new(RwLock::new(UndoStack::default())),
            matview_manager,
            profile_resolver,
            sql_transformers,
            _backend_keepalive: backend,
        })
    }

    /// Apply all registered SQL-level transformers to a SQL string.
    ///
    /// Returns the original string unchanged if parsing fails.
    pub fn apply_sql_transforms(&self, sql: &str) -> String {
        crate::storage::apply_sql_transforms(sql, &self.sql_transformers)
    }

    /// Get the database handle for direct database operations
    pub fn db_handle(&self) -> &DbHandle {
        &self.db_handle
    }

    /// Get the profile resolver for entity profile resolution
    pub fn profile_resolver(&self) -> &Arc<dyn crate::entity_profile::ProfileResolving> {
        &self.profile_resolver
    }

    /// Get the CDC broadcast sender for subscribing to change events
    pub fn cdc_broadcast(&self) -> &broadcast::Sender<BatchWithMetadata<RowChange>> {
        &self.cdc_broadcast
    }

    /// Get the matview manager for external callers that need to create watched queries.
    pub fn matview_manager(&self) -> &crate::sync::MatviewManager {
        &self.matview_manager
    }

    /// Access block-specific domain methods (rendering, layout, task ranking).
    pub fn blocks(&self) -> BlockDomain<'_> {
        BlockDomain::new(self)
    }

    /// Pre-create materialized views for the given SQL queries.
    ///
    /// This should be called during initialization, BEFORE any data loading or
    /// file watching starts. By pre-creating views:
    /// - Views start empty and are populated by IVM as data arrives
    /// - Later `watch_query` calls find existing views (no DDL needed)
    /// - No contention between view creation and IVM processing
    pub async fn preload_views(&self, sql_queries: &[&str]) -> Result<()> {
        // Drop stale watch_view_* matviews from previous sessions.
        // Turso IVM can produce incorrect results when matviews survive across
        // app restarts with different data (e.g., regenerated document UUIDs).
        self.matview_manager.drop_stale_views().await?;

        tracing::info!(
            "[BackendEngine] preload_views: pre-creating {} views",
            sql_queries.len()
        );
        for sql in sql_queries {
            let sql_with_params = Self::inline_parameters(sql, &HashMap::new());
            self.matview_manager.preload(&sql_with_params).await?;
        }
        tracing::info!("[BackendEngine] preload_views: completed");
        Ok(())
    }

    /// Compile a query in any supported language (prql, gql, sql) to final SQL.
    ///
    /// 1. Compile to raw SQL (unless already SQL)
    /// 2. Apply SQL-level transforms
    #[tracing::instrument(skip(self, query), fields(language = %language))]
    pub fn compile_to_sql(&self, query: &str, language: QueryLanguage) -> Result<String> {
        let raw_sql = match language {
            QueryLanguage::HolonPrql => self.compile_prql_to_raw_sql(query)?,
            QueryLanguage::HolonGql => self.compile_gql(query)?,
            QueryLanguage::HolonSql => query.to_string(),
        };
        Ok(self.apply_sql_transforms(&raw_sql))
    }

    /// Compile a PRQL query to raw SQL (no transforms applied).
    fn compile_prql_to_raw_sql(&self, prql: &str) -> Result<String> {
        let full_prql = format!("{}\n{}", PRQL_STDLIB, prql);
        let sql = prqlc::compile(&full_prql, &prqlc::Options::default().no_signature())
            .map_err(|e| anyhow::anyhow!("PRQL compilation failed: {}", e))?;
        Ok(sql)
    }

    /// Compile a GQL query to SQL.
    ///
    /// Uses a `GraphSchema` with mapped relational tables so existing tables
    /// (blocks, documents) are queryable as graph nodes via GQL, alongside
    /// the EAV tables for ad-hoc graph data.
    pub fn compile_gql(&self, gql: &str) -> Result<String> {
        let parsed = gql_parser::parse(gql)
            .map_err(|e| anyhow::anyhow!("GQL parse error: {}", e.message))?;
        let query = match parsed {
            gql_parser::QueryOrUnion::Query(q) => q,
            gql_parser::QueryOrUnion::Union(_) => {
                anyhow::bail!("UNION queries not yet supported in GQL")
            }
        };
        let schema = Self::build_graph_schema();
        let sql = gql_transform::transform(&query, &schema)
            .map_err(|e| anyhow::anyhow!("GQL transform error: {:?}", e))?;
        // GQL transform outputs :param syntax, convert to $param for inline_parameters
        Ok(Self::gql_params_to_dollar(&sql))
    }

    /// Convert GQL `:param` syntax to `$param` for compatibility with `inline_parameters`.
    fn gql_params_to_dollar(sql: &str) -> String {
        use std::fmt::Write;
        let mut result = String::with_capacity(sql.len());
        let mut chars = sql.chars().peekable();
        while let Some(c) = chars.next() {
            if c == ':' {
                if chars
                    .peek()
                    .map_or(false, |ch| ch.is_ascii_alphabetic() || *ch == '_')
                {
                    result.push('$');
                    while chars
                        .peek()
                        .map_or(false, |ch| ch.is_ascii_alphanumeric() || *ch == '_')
                    {
                        let _ = write!(result, "{}", chars.next().unwrap());
                    }
                } else {
                    result.push(c);
                }
            } else if c == '\'' {
                // Skip string literals — don't convert inside quoted strings
                result.push(c);
                while let Some(sc) = chars.next() {
                    result.push(sc);
                    if sc == '\'' {
                        break;
                    }
                }
            } else {
                result.push(c);
            }
        }
        result
    }

    /// Build the graph schema mapping relational tables as graph nodes/edges.
    ///
    /// - `:Block` → `blocks` table (id, parent_id, content, content_type, ...)
    /// - `:Document` → `documents` table (id, name, parent_id)
    /// - `:CurrentFocus` → `current_focus` materialized view (region, block_id, timestamp)
    /// - `:CHILD_OF` → FK edge: blocks.parent_id → blocks.id
    /// - `:IN_DOCUMENT` → FK edge: blocks.parent_id → documents.id
    /// - `:DOC_CHILD_OF` → FK edge: documents.parent_id → documents.id
    /// - `:FOCUSES_ON` → FK edge: current_focus.block_id → blocks.id
    ///
    /// Unlabeled nodes and edges fall back to EAV resolvers.
    fn build_graph_schema() -> gql_transform::resolver::GraphSchema {
        use gql_transform::resolver::*;

        let block_columns = vec![
            ColumnMapping {
                property_name: "id".into(),
                column_name: "id".into(),
            },
            ColumnMapping {
                property_name: "parent_id".into(),
                column_name: "parent_id".into(),
            },
            ColumnMapping {
                property_name: "content".into(),
                column_name: "content".into(),
            },
            ColumnMapping {
                property_name: "content_type".into(),
                column_name: "content_type".into(),
            },
            ColumnMapping {
                property_name: "source_language".into(),
                column_name: "source_language".into(),
            },
            ColumnMapping {
                property_name: "depth".into(),
                column_name: "depth".into(),
            },
            ColumnMapping {
                property_name: "sort_key".into(),
                column_name: "sort_key".into(),
            },
            ColumnMapping {
                property_name: "collapsed".into(),
                column_name: "collapsed".into(),
            },
            ColumnMapping {
                property_name: "completed".into(),
                column_name: "completed".into(),
            },
            ColumnMapping {
                property_name: "block_type".into(),
                column_name: "block_type".into(),
            },
            ColumnMapping {
                property_name: "properties".into(),
                column_name: "properties".into(),
            },
        ];

        let doc_columns = vec![
            ColumnMapping {
                property_name: "id".into(),
                column_name: "id".into(),
            },
            ColumnMapping {
                property_name: "name".into(),
                column_name: "name".into(),
            },
            ColumnMapping {
                property_name: "parent_id".into(),
                column_name: "parent_id".into(),
            },
            ColumnMapping {
                property_name: "sort_key".into(),
                column_name: "sort_key".into(),
            },
            ColumnMapping {
                property_name: "properties".into(),
                column_name: "properties".into(),
            },
        ];

        let mut nodes: std::collections::HashMap<String, Box<dyn NodeResolver>> =
            std::collections::HashMap::new();
        nodes.insert(
            "Block".into(),
            Box::new(MappedNodeResolver {
                table_name: "block".into(),
                id_col: "id".into(),
                label: "Block".into(),
                columns: block_columns,
            }),
        );
        nodes.insert(
            "Document".into(),
            Box::new(MappedNodeResolver {
                table_name: "document".into(),
                id_col: "id".into(),
                label: "Document".into(),
                columns: doc_columns,
            }),
        );
        nodes.insert(
            "CurrentFocus".into(),
            Box::new(MappedNodeResolver {
                table_name: "current_focus".into(),
                id_col: "region".into(),
                label: "CurrentFocus".into(),
                columns: vec![
                    ColumnMapping {
                        property_name: "region".into(),
                        column_name: "region".into(),
                    },
                    ColumnMapping {
                        property_name: "block_id".into(),
                        column_name: "block_id".into(),
                    },
                    ColumnMapping {
                        property_name: "timestamp".into(),
                        column_name: "timestamp".into(),
                    },
                ],
            }),
        );
        nodes.insert(
            "FocusRoot".into(),
            Box::new(MappedNodeResolver {
                table_name: "focus_roots".into(),
                id_col: "root_id".into(),
                label: "FocusRoot".into(),
                columns: vec![
                    ColumnMapping {
                        property_name: "region".into(),
                        column_name: "region".into(),
                    },
                    ColumnMapping {
                        property_name: "block_id".into(),
                        column_name: "block_id".into(),
                    },
                    ColumnMapping {
                        property_name: "root_id".into(),
                        column_name: "root_id".into(),
                    },
                ],
            }),
        );

        let mut edges: std::collections::HashMap<String, EdgeDef> =
            std::collections::HashMap::new();
        edges.insert(
            "CHILD_OF".into(),
            EdgeDef {
                source_label: Some("Block".into()),
                target_label: Some("Block".into()),
                resolver: Box::new(ForeignKeyEdgeResolver {
                    fk_table: "block".into(),
                    fk_column: "parent_id".into(),
                    target_table: "block".into(),
                    target_id_column: "id".into(),
                }),
            },
        );
        edges.insert(
            "FOCUSES_ON".into(),
            EdgeDef {
                source_label: Some("CurrentFocus".into()),
                target_label: Some("Block".into()),
                resolver: Box::new(ForeignKeyEdgeResolver {
                    fk_table: "current_focus".into(),
                    fk_column: "block_id".into(),
                    target_table: "block".into(),
                    target_id_column: "id".into(),
                }),
            },
        );

        GraphSchema {
            nodes,
            edges,
            default_node_resolver: Box::new(EavNodeResolver),
            default_edge_resolver: Box::new(EavEdgeResolver),
            raw_return: true,
        }
    }

    /// Inline parameter values directly into SQL (for materialized view definitions)
    ///
    /// Unlike bind_parameters which uses `?` placeholders, this function substitutes
    /// actual values into the SQL string. This is necessary for CREATE MATERIALIZED VIEW
    /// statements where the view definition must contain literal values, not parameters.
    ///
    /// Values are properly escaped/quoted:
    /// - Strings: 'escaped''quotes'
    /// - Numbers: literal
    /// - Null: NULL
    /// - Bool: 1/0
    fn inline_parameters(sql: &str, params: &HashMap<String, Value>) -> String {
        let mut result = String::with_capacity(sql.len());
        let mut chars = sql.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '$' {
                if let Some(&next_ch) = chars.peek() {
                    if next_ch.is_alphanumeric() || next_ch == '_' {
                        let mut param_name = String::new();
                        while let Some(&next_ch) = chars.peek() {
                            if next_ch.is_alphanumeric() || next_ch == '_' {
                                param_name.push(chars.next().unwrap());
                            } else {
                                break;
                            }
                        }

                        if let Some(value) = params.get(&param_name) {
                            result.push_str(&value_to_sql_literal(value));
                        } else {
                            result.push('$');
                            result.push_str(&param_name);
                        }
                    } else {
                        result.push('$');
                    }
                } else {
                    result.push('$');
                }
            } else {
                result.push(ch);
            }
        }

        result
    }

    /// Compute a deterministic view name for a given SQL query and parameters.
    ///
    /// This is used to create materialized views with consistent names, allowing
    /// us to create the view first and then query it for initial data.
    /// Bind context parameters to the parameter map
    ///
    /// Adds `$context_id`, `$context_parent_id`, and `$context_path_prefix` parameters
    /// based on QueryContext. None values are bound as Value::Null.
    fn bind_context_params(&self, params: &mut HashMap<String, Value>, context: &QueryContext) {
        match &context.current_block_id {
            Some(id) => {
                params.insert(
                    "context_id".to_string(),
                    Value::String(id.as_str().to_string()),
                );
            }
            None => {
                params.insert("context_id".to_string(), Value::Null);
            }
        }
        match &context.context_parent_id {
            Some(id) => {
                params.insert(
                    "context_parent_id".to_string(),
                    Value::String(id.as_str().to_string()),
                );
            }
            None => {
                params.insert("context_parent_id".to_string(), Value::Null);
            }
        }
        match &context.context_path_prefix {
            Some(prefix) => {
                params.insert(
                    "context_path_prefix".to_string(),
                    Value::String(prefix.clone()),
                );
            }
            None => {
                // No path prefix means descendants queries won't match anything
                // This is intentional - use for_block_with_path() to enable descendants
                params.insert(
                    "context_path_prefix".to_string(),
                    Value::String("__NO_PATH__/".to_string()),
                );
            }
        }
    }

    /// Execute a SQL query and return the result set
    ///
    /// Supports parameter binding by replacing `$param_name` placeholders with actual values.
    /// Parameters are bound safely using SQL parameter binding to prevent SQL injection.
    ///
    /// # Arguments
    /// * `sql` - The SQL query to execute
    /// * `params` - Parameters to bind to the query
    /// * `context` - Optional query context for virtual table parameter binding
    #[tracing::instrument(skip(self, sql, params, context))]
    pub async fn execute_query(
        &self,
        sql: String,
        mut params: HashMap<String, Value>,
        context: Option<QueryContext>,
    ) -> Result<Vec<HashMap<String, Value>>> {
        // Always bind context params (using NULL if no context provided).
        // This enables stdlib virtual tables like `from children` to compile even without context.
        let ctx = context.unwrap_or_else(QueryContext::root);
        self.bind_context_params(&mut params, &ctx);

        // Retry with fresh connections to handle "Database schema changed" errors
        // that occur when DDL operations race with queries during startup.
        // Fresh connections don't have stale prepared statement caches.
        // db_handle used directly
        let mut last_error = None;
        for attempt in 0..5 {
            // On first attempt, use normal connection. On retries, use fresh connection
            // to avoid stale prepared statement caches.
            let result = if attempt == 0 {
                self.db_handle.query(&sql, params.clone()).await
            } else {
                self.db_handle.query(&sql, params.clone()).await
            };
            match result {
                Ok(result) => return Ok(result),
                Err(e) => {
                    let err_str = format!("{:?}", e);
                    let is_schema_error = err_str.contains("Database schema changed");
                    if is_schema_error && attempt < 4 {
                        tracing::debug!(
                            "[execute_query] Retry {} due to schema change: {}",
                            attempt + 1,
                            err_str
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(50 * (1 << attempt)))
                            .await;
                        last_error = Some(e);
                    } else {
                        return Err(anyhow::anyhow!("SQL execution failed: {}", e));
                    }
                }
            }
        }
        Err(anyhow::anyhow!(
            "SQL execution failed after retries: {:?}",
            last_error
        ))
    }

    /// Watch a query for changes via CDC streaming
    ///
    /// Returns a stream of RowChange events from the underlying database.
    /// The CDC connection is stored in the BackendEngine to keep it alive.
    ///
    /// Note: The SQL should include `_change_origin` column for CDC trace propagation.
    /// When using `compile_query` or `query_and_watch`, this is handled automatically
    /// by the SQL transformers.
    ///
    /// # Arguments
    /// * `sql` - The SQL query to watch
    /// * `params` - Parameters to bind to the query
    /// * `context` - Optional query context for virtual table parameter binding
    pub async fn watch_query(
        &self,
        sql: String,
        mut params: HashMap<String, Value>,
        context: Option<QueryContext>,
    ) -> Result<RowChangeStream> {
        let ctx = context.unwrap_or_else(QueryContext::root);
        self.bind_context_params(&mut params, &ctx);

        let sql_with_params = Self::inline_parameters(&sql, &params);
        let view_name = self.matview_manager.ensure_view(&sql_with_params).await?;
        Ok(self.matview_manager.subscribe_cdc(&view_name))
    }

    /// Execute a SQL query, set up CDC streaming, and return initial data + change stream.
    ///
    /// # Arguments
    /// * `sql` - The SQL query to execute and watch
    /// * `params` - Parameters to bind to the query
    /// * `context` - Optional query context for virtual table parameter binding
    ///
    /// # Returns
    /// A tuple containing:
    /// - `WidgetSpec`: Contains data (actions empty for regular queries)
    /// - `RowChangeStream`: Stream of ongoing changes to the query results
    #[tracing::instrument(skip(self, sql, params, context))]
    pub async fn query_and_watch(
        &self,
        sql: String,
        params: HashMap<String, Value>,
        context: Option<QueryContext>,
    ) -> Result<(WidgetSpec, RowChangeStream)> {
        let transformed_sql = self.apply_sql_transforms(&sql);
        tracing::debug!("[BackendEngine] SQL:\n{}", transformed_sql);

        let ctx = context.clone().unwrap_or_else(QueryContext::root);

        // Inline params to get the final SQL for the matview
        let mut params_with_context = params.clone();
        self.bind_context_params(&mut params_with_context, &ctx);
        let sql_with_params = Self::inline_parameters(&transformed_sql, &params_with_context);

        // Ensure view exists, subscribe to CDC, and query initial data
        let view_name = self.matview_manager.ensure_view(&sql_with_params).await?;
        let change_stream = self.matview_manager.subscribe_cdc(&view_name);

        let mut data = None;
        for attempt in 0..10 {
            match self.matview_manager.query_view(&view_name).await {
                Ok(results) => {
                    data = Some(results);
                    break;
                }
                Err(e) => {
                    let err_str = format!("{:?}", e);
                    let is_retryable = err_str.contains("no such table")
                        || err_str.contains("Database schema changed");
                    if is_retryable && attempt < 9 {
                        tracing::debug!(
                            "[query_and_watch] Retryable error (attempt {}): {}",
                            attempt + 1,
                            err_str
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(
                            50 * (1 << attempt.min(4)),
                        ))
                        .await;
                        continue;
                    }
                    return Err(anyhow::anyhow!(
                        "Failed to query matview for initial data: {}",
                        e
                    ));
                }
            }
        }
        let data = data.ok_or_else(|| anyhow::anyhow!("Failed to query matview after retries"))?;

        let mut widget_spec = WidgetSpec::from_rows(data);

        let profile_ctx = ctx.profile_context.clone().unwrap_or_default();
        self.attach_row_profiles(&mut widget_spec, &profile_ctx);

        Ok((widget_spec, change_stream))
    }

    /// Attach resolved entity profiles to a WidgetSpec's rows.
    fn attach_row_profiles(
        &self,
        widget_spec: &mut WidgetSpec,
        profile_ctx: &crate::entity_profile::ProfileContext,
    ) {
        use crate::entity_profile::ProfileResolving;

        let mut spec_cache: HashMap<String, holon_api::render_types::RowProfile> = HashMap::new();

        for row in &mut widget_spec.data {
            let (profile, computed) = self
                .profile_resolver
                .resolve_with_computed(&row.data, profile_ctx);
            if profile.name == "fallback" {
                continue;
            }
            // Insert computed field values into row data so the frontend can access them via col()
            for (key, value) in computed {
                row.data.insert(key, value);
            }
            let spec = spec_cache.entry(profile.name.clone()).or_insert_with(|| {
                holon_api::render_types::RowProfile {
                    name: profile.name.clone(),
                    render: profile.render.clone(),
                    operations: profile.operations.clone(),
                }
            });
            row.profile = Some(spec.clone());
        }
    }

    /// Execute a block operation
    ///
    /// This method provides a clean interface for executing operations without exposing
    /// the internal TursoBackend. It handles locking and passes the current UI state.
    ///
    /// # Arguments
    /// * `op_name` - Name of the operation to execute (e.g., "indent", "outdent", "move_block")
    /// * `params` - Parameters for the operation (typically includes block ID and operation-specific fields)
    ///
    /// # Returns
    /// Result indicating success or failure. On success, UI should re-query to get updated data.
    ///
    /// # Example
    /// ```no_run
    /// use std::collections::HashMap;
    /// use holon::api::backend_engine::BackendEngine;
    /// use holon_api::Value;
    ///
    /// # async fn example() -> anyhow::Result<()> {
    /// let engine = BackendEngine::new_in_memory().await?;
    ///
    /// let mut params = HashMap::new();
    /// params.insert("id".to_string(), Value::String("block-1".to_string()));
    ///
    /// engine.execute_operation("indent", params).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn execute_operation(
        &self,
        entity_name: &str,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<Option<Value>> {
        use tracing::Instrument;
        use tracing::info;

        // Create tracing span that will be bridged to OpenTelemetry
        // Use .instrument() to maintain context across async boundaries
        let span = tracing::span!(
            tracing::Level::INFO,
            "backend.execute_operation",
            "operation.entity" = entity_name,
            "operation.name" = op_name
        );

        async {
            info!(
                "[BackendEngine] execute_operation: entity={}, op={}, params={:?}",
                entity_name, op_name, params
            );

            // Build original operation for undo stack
            let original_op = Operation::new(
                entity_name,
                op_name,
                "", // display_name will be set from OperationDescriptor if needed
                params.clone(),
            );

            // Execute via dispatcher using entity_name
            // Span context will be propagated via tracing-opentelemetry bridge
            let operation_result = self.dispatcher
                .execute_operation(entity_name, op_name, params)
                .await;

            match &operation_result {
                Ok(result) => {
                    match &result.undo {
                        UndoAction::Undo(_) => {
                            info!(
                                "[BackendEngine] execute_operation succeeded: entity={}, op={} (inverse operation available)",
                                entity_name, op_name
                            );
                        }
                        UndoAction::Irreversible => {
                            info!(
                                "[BackendEngine] execute_operation succeeded: entity={}, op={} (no inverse operation)",
                                entity_name, op_name
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(
                        "[BackendEngine] Operation '{}' on entity '{}' failed: {}",
                        op_name, entity_name, e
                    );
                }
            }

            // If operation succeeded and has an inverse, push to undo stack
            if let Ok(result) = &operation_result {
                if let UndoAction::Undo(inverse_op) = &result.undo {
                    let mut undo_stack = self.undo_stack.write().await;
                    undo_stack.push(original_op, inverse_op.clone());
                }
            }

            operation_result.map(|r| r.response).map_err(|e| {
                anyhow::anyhow!(
                    "Operation '{}' on entity '{}' failed: {}",
                    op_name,
                    entity_name,
                    e
                )
            })
        }
        .instrument(span)
        .await
    }

    /// Undo the last operation
    ///
    /// Executes the inverse operation from the undo stack and pushes it to the redo stack.
    /// Returns true if an operation was undone, false if the undo stack is empty.
    pub async fn undo(&self) -> Result<bool> {
        // Pop the inverse operation from undo stack (automatically moves to redo stack)
        let inverse_op = {
            let mut undo_stack = self.undo_stack.write().await;
            undo_stack
                .pop_for_undo()
                .ok_or_else(|| anyhow::anyhow!("Nothing to undo"))?
        };

        // Execute the inverse operation
        let operation_result = self
            .dispatcher
            .execute_operation(
                inverse_op.entity_name.as_str(),
                &inverse_op.op_name,
                inverse_op.params.clone(),
            )
            .await
            .map_err(|e| anyhow::anyhow!("Failed to execute undo operation: {}", e))?;

        // Update the redo stack with the new inverse operation
        // The UndoStack already moved (inverse, original) to redo stack,
        // but we need to update it with the new inverse we got from execution
        if let UndoAction::Undo(new_inverse_op) = operation_result.undo {
            let mut undo_stack = self.undo_stack.write().await;
            undo_stack.update_redo_top(new_inverse_op);
        }

        Ok(true)
    }

    /// Redo the last undone operation
    ///
    /// Executes the inverse of the last undone operation and pushes it back to the undo stack.
    /// Returns true if an operation was redone, false if the redo stack is empty.
    pub async fn redo(&self) -> Result<bool> {
        // Pop the operation to redo from redo stack (automatically moves back to undo stack)
        let operation_to_redo = {
            let mut undo_stack = self.undo_stack.write().await;
            undo_stack
                .pop_for_redo()
                .ok_or_else(|| anyhow::anyhow!("Nothing to redo"))?
        };

        // Execute the operation to redo
        let operation_result = self
            .dispatcher
            .execute_operation(
                operation_to_redo.entity_name.as_str(),
                &operation_to_redo.op_name,
                operation_to_redo.params.clone(),
            )
            .await
            .map_err(|e| anyhow::anyhow!("Failed to execute redo operation: {}", e))?;

        // Update the undo stack with the new inverse operation
        // The UndoStack already moved (inverse, operation_to_redo) back to undo stack,
        // but we need to update it with the new inverse we got from execution
        if let UndoAction::Undo(new_inverse_op) = operation_result.undo {
            let mut undo_stack = self.undo_stack.write().await;
            undo_stack.update_undo_top(new_inverse_op);
        }

        Ok(true)
    }

    /// Check if undo is available
    pub async fn can_undo(&self) -> bool {
        self.undo_stack.read().await.can_undo()
    }

    /// Check if redo is available
    pub async fn can_redo(&self) -> bool {
        self.undo_stack.read().await.can_redo()
    }

    /// Register a custom OperationProvider
    ///
    /// This allows registering additional operation providers for entity types.
    /// Operations are automatically discovered via the OperationProvider trait.
    ///
    /// # Example
    /// ```no_run
    /// use std::sync::Arc;
    /// use holon::api::backend_engine::BackendEngine;
    /// use holon::core::datasource::OperationProvider;
    ///
    /// # async fn example() -> anyhow::Result<()> {
    /// let engine = BackendEngine::new_in_memory().await?;
    ///
    /// // Register custom provider
    /// // engine.register_provider("my-entity", my_provider).await?;
    /// # Ok(())
    /// # }
    /// ```

    pub async fn available_operations(&self, entity_name: &str) -> Vec<OperationDescriptor> {
        self.dispatcher
            .operations()
            .into_iter()
            .filter(|op| op.entity_name == entity_name)
            .collect()
    }

    pub async fn has_operation(&self, entity_name: &str, op_name: &str) -> bool {
        self.dispatcher
            .operations()
            .into_iter()
            .any(|op| op.entity_name == entity_name && op.name == op_name)
    }

    /// Map a table name to an entity name
    ///
    /// This mapping is used during query compilation to determine which
    /// entity type operations are available for a given table.
    ///
    /// # Arguments
    /// * `table_name` - Database table name (e.g., "todoist_task", "logseq_block")
    /// * `entity_name` - Entity identifier (e.g., "todoist-task", "logseq-block")
    pub async fn map_table_to_entity(&self, table_name: String, entity_name: String) {
        let mut map = self.table_to_entity_map.write().await;
        map.insert(table_name, entity_name);
    }

    /// Get the entity name for a table
    ///
    /// # Arguments
    /// * `table_name` - Database table name
    ///
    /// # Returns
    /// `Some(entity_name)` if mapped, `None` otherwise
    pub async fn get_entity_for_table(&self, table_name: &str) -> Option<String> {
        let map = self.table_to_entity_map.read().await;
        map.get(table_name).cloned()
    }

    /// Get a clone of the operation dispatcher Arc
    ///
    /// This allows querying available operations without mutating the dispatcher.
    pub fn get_dispatcher(&self) -> Arc<OperationDispatcher> {
        self.dispatcher.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::sql_operation_provider::SqlOperationProvider;
    use crate::di::test_helpers::{create_test_engine, create_test_engine_with_providers};
    use std::sync::Arc;

    #[test]
    fn prql_stdlib_compiles_successfully() {
        let full_prql = format!("{}\nfrom block", PRQL_STDLIB);
        prqlc::compile(&full_prql, &prqlc::Options::default().no_signature())
            .expect("PRQL_STDLIB should compile without errors");
    }

    #[test]
    fn test_inline_parameters() {
        let mut params = HashMap::new();
        params.insert(
            "context_id".to_string(),
            Value::String("block-123".to_string()),
        );
        params.insert("context_parent_id".to_string(), Value::Null);
        params.insert("num".to_string(), Value::Integer(42));
        params.insert("flag".to_string(), Value::Boolean(true));

        // Test string parameter
        let sql = "SELECT * FROM block WHERE id = $context_id";
        let result = BackendEngine::inline_parameters(sql, &params);
        assert_eq!(result, "SELECT * FROM block WHERE id = 'block-123'");

        // Test NULL parameter
        let sql = "SELECT * FROM block WHERE parent_id = $context_parent_id";
        let result = BackendEngine::inline_parameters(sql, &params);
        assert_eq!(result, "SELECT * FROM block WHERE parent_id = NULL");

        // Test integer parameter
        let sql = "SELECT * FROM block WHERE count = $num";
        let result = BackendEngine::inline_parameters(sql, &params);
        assert_eq!(result, "SELECT * FROM block WHERE count = 42");

        // Test boolean parameter
        let sql = "SELECT * FROM block WHERE active = $flag";
        let result = BackendEngine::inline_parameters(sql, &params);
        assert_eq!(result, "SELECT * FROM block WHERE active = 1");

        // Test multiple parameters
        let sql = "SELECT * FROM block WHERE id = $context_id AND parent_id = $context_parent_id";
        let result = BackendEngine::inline_parameters(sql, &params);
        assert_eq!(
            result,
            "SELECT * FROM block WHERE id = 'block-123' AND parent_id = NULL"
        );

        // Test unknown parameter is preserved
        let sql = "SELECT * FROM block WHERE id = $unknown_param";
        let result = BackendEngine::inline_parameters(sql, &params);
        assert_eq!(result, "SELECT * FROM block WHERE id = $unknown_param");

        // Test SQL injection prevention (quotes are escaped)
        let mut params_with_quote = HashMap::new();
        params_with_quote.insert("name".to_string(), Value::String("O'Brien".to_string()));
        let sql = "SELECT * FROM users WHERE name = $name";
        let result = BackendEngine::inline_parameters(sql, &params_with_quote);
        assert_eq!(result, "SELECT * FROM users WHERE name = 'O''Brien'");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_render_engine_creation() {
        let engine = create_test_engine().await;
        assert!(engine.is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_compile_to_sql() {
        let engine = create_test_engine().await.unwrap();

        let prql = "from block | select {id, content}";
        let result = engine.compile_to_sql(prql, QueryLanguage::HolonPrql);
        assert!(result.is_ok());

        let sql = result.unwrap();
        assert!(sql.to_uppercase().contains("SELECT"));
        assert!(sql.to_uppercase().contains("FROM"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_execute_query_with_parameters() {
        let engine = create_test_engine().await.unwrap();

        // Create a test table and insert data using db_handle
        let _ = engine
            .db_handle()
            .execute_ddl("DROP TABLE IF EXISTS test_blocks")
            .await;
        engine
            .db_handle()
            .execute_ddl(
                "CREATE TABLE test_blocks (id TEXT PRIMARY KEY, title TEXT, depth INTEGER)",
            )
            .await
            .unwrap();

        engine
            .db_handle()
            .execute(
                "INSERT INTO test_blocks (id, title, depth) VALUES ('block-1', 'Test Block', 0)",
                vec![],
            )
            .await
            .unwrap();

        engine
            .db_handle()
            .execute(
                "INSERT INTO test_blocks (id, title, depth) VALUES ('block-2', 'Nested Block', 1)",
                vec![],
            )
            .await
            .unwrap();

        // Test query with parameter binding
        let mut params = HashMap::new();
        params.insert("min_depth".to_string(), Value::Integer(0));

        let sql = "SELECT id, title, depth FROM test_blocks WHERE depth >= $min_depth ORDER BY id";
        let results = engine
            .execute_query(sql.to_string(), params, None)
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].get("id").unwrap().as_string(), Some("block-1"));
        assert_eq!(results[1].get("id").unwrap().as_string(), Some("block-2"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_parameter_binding() {
        let engine = create_test_engine().await.unwrap();

        // Create table and insert data using db_handle
        let _ = engine
            .db_handle()
            .execute_ddl("DROP TABLE IF EXISTS users")
            .await;
        engine
            .db_handle()
            .execute_ddl("CREATE TABLE users (id TEXT, name TEXT, age INTEGER)")
            .await
            .unwrap();

        engine
            .db_handle()
            .execute(
                "INSERT INTO users VALUES ('u1', 'Alice', 30), ('u2', 'Bob', 25), ('u3', 'Charlie', 35)",
                vec![],
            )
            .await
            .unwrap();

        // Test multiple parameters
        let mut params = HashMap::new();
        params.insert("min_age".to_string(), Value::Integer(25));
        params.insert("max_age".to_string(), Value::Integer(35));

        let sql =
            "SELECT name, age FROM users WHERE age >= $min_age AND age <= $max_age ORDER BY age";
        let results = engine
            .execute_query(sql.to_string(), params, None)
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].get("name").unwrap().as_string(), Some("Bob"));
        assert_eq!(results[2].get("name").unwrap().as_string(), Some("Charlie"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_execute_operation() {
        let engine = create_test_engine_with_providers(":memory:".into(), |module| {
            module.with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                Arc::new(SqlOperationProvider::new(
                    db_handle,
                    "test_item".to_string(),
                    "test_item".to_string(),
                    "test_item".to_string(),
                ))
            })
        })
        .await
        .unwrap();

        // Create test table using db_handle
        engine
            .db_handle()
            .execute_ddl(
                "CREATE TABLE test_item (id TEXT PRIMARY KEY, content TEXT, completed BOOLEAN)",
            )
            .await
            .unwrap();

        engine
            .db_handle()
            .execute(
                "INSERT INTO test_item (id, content, completed) VALUES ('item-1', 'Test task', 0)",
                vec![],
            )
            .await
            .unwrap();

        // Execute operation to update completed field
        let mut params = HashMap::new();
        params.insert("id".to_string(), Value::String("item-1".to_string()));
        params.insert("field".to_string(), Value::String("completed".to_string()));
        params.insert("value".to_string(), Value::Boolean(true));

        let result = engine
            .execute_operation("test_item", "set_field", params)
            .await;
        assert!(result.is_ok(), "Operation should succeed: {:?}", result);

        // Verify the update
        let sql = "SELECT id, completed FROM test_item WHERE id = 'item-1'";
        let results = engine
            .execute_query(sql.to_string(), HashMap::new(), None)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].get("id").unwrap().as_string(), Some("item-1"));

        // SQLite stores booleans as integers (0/1), so check for Integer value
        match results[0].get("completed").unwrap() {
            Value::Integer(i) => assert_eq!(*i, 1, "Expected completed=1 (true)"),
            Value::Boolean(b) => assert!(b, "Expected completed=true"),
            other => panic!("Unexpected value type for completed: {:?}", other),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_execute_operation_failure() {
        let engine = create_test_engine().await.unwrap();

        // Try to execute non-existent operation
        let params = HashMap::new();
        let result = engine
            .execute_operation("block", "nonexistent", params)
            .await;

        assert!(result.is_err(), "Should fail for non-existent operation");
        let error_msg = result.unwrap_err().to_string();
        assert!(
            error_msg.contains("nonexistent"),
            "Error should mention operation name"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_register_custom_operation() {
        // Use the provider factory pattern so the provider gets the correct db_handle
        let engine = create_test_engine_with_providers(":memory:".into(), |module| {
            module.with_operation_provider_factory(|backend| {
                // Get db_handle from backend using block_in_place to avoid blocking issues
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                Arc::new(SqlOperationProvider::new(
                    db_handle,
                    "block".to_string(),
                    "block".to_string(),
                    "block".to_string(),
                ))
            })
        })
        .await
        .unwrap();

        // Verify operations are available
        let ops = engine.available_operations("block").await;
        assert!(!ops.is_empty(), "Should have operations available");
        // Verify we get OperationDescriptor objects with proper properties
        assert!(ops.iter().all(|op| op.entity_name == "block"));
        assert!(ops.iter().any(|op| !op.name.is_empty()));
    }
}
