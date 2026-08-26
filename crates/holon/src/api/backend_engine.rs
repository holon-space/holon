use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use holon_api::BatchWithMetadata;
use holon_api::EntityName;
use holon_api::OperationDescriptor;
use holon_api::QueryContext;
use holon_api::QueryLanguage;
use holon_api::Value;
use holon_core::storage::types::StorageEntity;
use tokio::sync::RwLock;
use tokio::sync::broadcast;

use crate::api::operation_dispatcher::OperationDispatcher;
use crate::api::operation_engine::DispatchingOperationEngine;
use crate::api::operation_engine::OperationEngine as _;
use crate::storage::DbHandle;
use crate::storage::SqlTransformer;
use crate::storage::sql_utils::rewrite_named_params;
use crate::storage::sql_utils::value_to_sql_literal;
use crate::storage::turso::RowChange;
use crate::storage::turso::RowChangeStream;

/// PRQL stdlib defining virtual tables for hierarchical queries
///
/// Note: When $context_id is NULL, PRQL generates `parent_id = NULL` which is
/// always false in SQL. The `children` virtual table should be used with
/// QueryContext::for_block() which sets a non-NULL context_id.
///
/// The `descendants` virtual table uses `block_with_path` materialized
/// view with path prefix matching. This enables efficient tree traversal using
/// precomputed hierarchical paths.
///
/// Note: We use `block_with_path` for descendants rather than PRQL's `loop`
/// because `let descendants = (... loop ...)` creates nested CTEs (outer CTE
/// for `let`, inner recursive CTE for `loop`) which prqlc doesn't flatten. The
/// path-prefix approach is also more efficient since `block_with_path` is a
/// pre-existing materialized view.
const PRQL_STDLIB: &str = include_str!("../../sql/prql_stdlib.prql");

use crate::api::block_domain::BlockDomain;

/// Main render engine managing database, query compilation, and operations
pub struct BackendEngine {
    /// Handle for all database operations (query, execute, DDL, CDC
    /// subscriptions)
    db_handle: DbHandle,
    /// Operation dispatcher for routing operations
    dispatcher: Arc<OperationDispatcher>,
    /// Maps table names to entity names
    table_to_entity_map: Arc<RwLock<HashMap<String, String>>>,
    /// The operation-execution capability (dispatch + undo/redo over the same
    /// `dispatcher`). Owns the per-session undo stack; the Turso engine's
    /// operation methods delegate here so dispatch/undo/redo logic lives in one
    /// place shared with the no-Turso wiring.
    op_engine: DispatchingOperationEngine,
    /// Manages materialized view lifecycle (creation, CDC, querying).
    matview_manager: crate::sync::MatviewManager,
    /// Entity profile resolver for per-row render + operation resolution
    profile_resolver: Arc<dyn crate::entity_profile::ProfileResolving>,
    /// SQL-level transformers applied after compilation (entity_name,
    /// _change_origin, json_agg)
    sql_transformers: Vec<Box<dyn SqlTransformer>>,
    /// GQL graph schema registry — mutable to support runtime entity
    /// registration (MCP).
    graph_schema_registry:
        Arc<std::sync::RwLock<crate::storage::graph_schema::GraphSchemaRegistry>>,
    /// Cached GQL graph schema, rebuilt from registry on mutation.
    graph_schema_cache: Arc<std::sync::RwLock<gql_transform::resolver::GraphSchema>>,
    /// Advice-rule compilation/runtime status (ADR 0022). Written by the advice
    /// reconciler task, read by the UI watcher so a broken rule renders its
    /// error in place. Empty until an advice reconciler is installed (see
    /// `create_initialized_engine`).
    advice_status: holon_advice::AdviceRuleStatusHandle,
    /// Reactive-rule (ADR 0024 WP3) compilation/runtime status. Written by the
    /// action watcher (deprecation / parse / compile / exec outcomes), read by
    /// the render path and MCP so a broken or deprecated rule surfaces its
    /// error in place. Empty until action watchers run.
    rule_status: crate::api::rule_status::RuleStatusHandle,
    /// ADR 0032 §2 — the `holon_rule` watcher's acceptance verdicts, the rule
    /// half of [`Self::derived_net`]'s source union. Written by the watcher's
    /// discovery loop, read on every derive.
    accepted_rules: crate::api::accepted_rules::AcceptedRuleHandle,
    /// Keeps the advice reconciler's background tasks alive (mirrors how the
    /// profile watcher / `advice reconciler` stay alive by being held on
    /// the engine). `None` in configs that never install one (tests,
    /// no-advice sessions).
    _advice_reconciler: Option<Arc<crate::sync::AdviceReconcilerHandle>>,
    /// Keeps the clock scheduler's ticking task alive (ADR 0024 P5,
    /// time-as-data). `None` until installed in
    /// `create_initialized_engine`; the boot guard there fails loud if it
    /// stays `None`.
    _clock_scheduler: Option<Arc<crate::sync::clock_scheduler::ClockSchedulerHandle>>,
}

impl BackendEngine {
    /// Create BackendEngine from dependencies (for dependency injection)
    ///
    /// Takes a `DbHandle` (for all database operations, DDL, and CDC
    /// subscriptions), a dispatcher, profile resolver, and SQL
    /// transformers.
    ///
    /// The database actor stays alive as long as any `DbHandle` clone exists,
    /// since `DbHandle` holds an `mpsc::Sender` to the actor's command channel.
    pub fn new(
        db_handle: DbHandle,
        dispatcher: Arc<OperationDispatcher>,
        profile_resolver: Arc<dyn crate::entity_profile::ProfileResolving>,
        sql_transformers: Vec<Box<dyn SqlTransformer>>,
        graph_schema_registry: crate::storage::graph_schema::GraphSchemaRegistry,
    ) -> Result<Self> {
        let ddl_mutex = Arc::new(tokio::sync::Mutex::new(()));
        let matview_manager = crate::sync::MatviewManager::new(db_handle.clone(), ddl_mutex);
        let graph_schema = graph_schema_registry.clone().build();
        // Wire the op/effect history relation (C2b): a Turso-backed, disclosed
        // ephemeral cache over this engine's db handle. The store computes its
        // own honest rebuild fidelity (`HistoryFidelity::Partial`) — no caller
        // asserts it. Org-standalone (no-Turso) wirings get
        // `DegradedHistoryStore` instead.
        let history = Arc::new(crate::api::history_store::TursoHistoryStore::new(
            db_handle.clone(),
        ));
        let op_engine = DispatchingOperationEngine::new(dispatcher.clone())
            .with_history_store(history)
            .with_template_source(Arc::new(
                crate::api::template_source::TursoTemplateSource::new(db_handle.clone()),
            ));
        Ok(Self {
            db_handle,
            dispatcher,
            table_to_entity_map: Arc::new(RwLock::new(HashMap::new())),
            op_engine,
            matview_manager,
            profile_resolver,
            sql_transformers,
            graph_schema_registry: Arc::new(std::sync::RwLock::new(graph_schema_registry)),
            graph_schema_cache: Arc::new(std::sync::RwLock::new(graph_schema)),
            advice_status: holon_advice::AdviceRuleStatusHandle::new(),
            rule_status: crate::api::rule_status::RuleStatusHandle::new(),
            accepted_rules: crate::api::accepted_rules::AcceptedRuleHandle::new(),
            _advice_reconciler: None,
            _clock_scheduler: None,
        })
    }

    /// The reactive-rule status map (ADR 0024 WP3) — the action watcher writes
    /// deprecation / parse / compile / exec outcomes; the render path reads it.
    pub fn rule_status(&self) -> &crate::api::rule_status::RuleStatusHandle {
        &self.rule_status
    }

    /// The `holon_rule` acceptance registry (ADR 0032 §2) — the watcher
    /// publishes its verdict per discovered rule block; the net derivation
    /// reads it.
    pub fn accepted_rules(&self) -> &crate::api::accepted_rules::AcceptedRuleHandle {
        &self.accepted_rules
    }

    /// The advice-rule status map (ADR 0022) — read by the UI watcher to
    /// replace a broken rule block's render with its error.
    pub fn advice_status(&self) -> &holon_advice::AdviceRuleStatusHandle {
        &self.advice_status
    }

    /// Install the advice reconciler: share the status handle the reconciler
    /// writes to and hold its keep-alive handle. Called once during engine
    /// initialization.
    pub fn install_advice_reconciler(
        &mut self,
        status: holon_advice::AdviceRuleStatusHandle,
        handle: crate::sync::AdviceReconcilerHandle,
    ) {
        self.advice_status = status;
        self._advice_reconciler = Some(Arc::new(handle));
    }

    /// Install the clock scheduler (ADR 0024 P5): hold its keep-alive handle so
    /// the day-rollover ticking task survives. Called once during engine
    /// initialization.
    pub fn install_clock_scheduler(
        &mut self,
        handle: crate::sync::clock_scheduler::ClockSchedulerHandle,
    ) {
        self._clock_scheduler = Some(Arc::new(handle));
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
        self.db_handle.cdc_broadcast()
    }

    /// Register a cache table as FDW-backed so that `ensure_view` primes the
    /// cache.
    pub async fn register_fdw_table(&self, cache_table: &str) {
        self.matview_manager.register_fdw_table(cache_table).await;
    }

    /// Set the matview hook called after FDW cache priming.
    pub async fn set_matview_hook(&self, hook: Arc<dyn holon_core::MatviewHook>) {
        self.matview_manager.set_hook(hook).await;
    }

    /// Drop all `watch_view_*` matviews. Used by `full_sync` to force fresh
    /// recreation.
    pub async fn drop_stale_matviews(&self) -> Result<()> {
        self.matview_manager.drop_stale_views().await
    }

    /// Snapshot of (cache_hits, exists_calls, ddl_creates) from the matview
    /// manager.
    pub fn matview_cache_metrics(&self) -> (u64, u64, u64) {
        self.matview_manager.cache_metrics()
    }

    /// Set up a CDC-driven view: ensure_view + initial query + CDC stream.
    /// Used by tests to build `LiveData` instances on top of arbitrary SELECTs.
    pub async fn watch_view(&self, sql: &str) -> Result<crate::sync::WatchResult> {
        self.matview_manager.watch(sql).await
    }

    /// Subscribe to CDC for the given SQL query.
    pub async fn subscribe_sql(&self, sql: &str) -> Result<RowChangeStream> {
        if std::env::var("HOLON_TRACE_VIEWS").is_ok() {
            let view_name_preview = crate::sync::MatviewManager::compute_view_name(sql);
            tracing::warn!(
                view_name = %view_name_preview,
                sql = %sql,
                "[diag-cdc-leak] subscribe_sql: SQL → view"
            );
        }
        let view_name = self.matview_manager.ensure_view(sql).await?;
        self.matview_manager.subscribe_cdc(&view_name).await
    }

    /// Register an entity type at runtime (e.g., from MCP integration).
    ///
    /// Adds the type to the persistent registry and rebuilds the cached
    /// `GraphSchema` so subsequent GQL queries can reference the new entity.
    pub fn register_entity_type(&self, type_def: holon_api::TypeDefinition) {
        let mut registry = self
            .graph_schema_registry
            .write()
            .expect("graph_schema_registry poisoned");
        registry.register_type(type_def);
        let new_schema = registry.clone().build();
        let mut cache = self
            .graph_schema_cache
            .write()
            .expect("graph_schema_cache poisoned");
        *cache = new_schema;
    }

    /// Access block-specific domain methods (rendering, layout, task ranking).
    pub fn blocks(&self) -> BlockDomain<'_> {
        BlockDomain::new(self)
    }

    /// Local, non-syncing UI state (the `local_ui_state` table). Per-device
    /// view choices live here, never on replicated block tables (ADR 0025);
    /// slot queries COALESCE these overrides over the synced choice. Lost on
    /// DB rebuild — disclosed (C2b ephemeral-cache doctrine).
    pub fn local_state(&self) -> crate::storage::local_state::LocalStateStore {
        crate::storage::local_state::LocalStateStore::new(self.db_handle.clone())
    }

    /// Ensure the local-UI-state table exists. Called once during DI init.
    pub async fn ensure_local_state(&self) -> Result<()> {
        crate::storage::local_state::ensure_local_ui_state(&self.db_handle).await
    }

    /// Pre-create materialized views for the given SQL queries.
    ///
    /// This should be called during initialization, BEFORE any data loading or
    /// file watching starts. By pre-creating views:
    /// - Views start empty and are populated by IVM as data arrives
    /// - Later `watch_query` calls find existing views (no DDL needed)
    /// - No contention between view creation and IVM processing
    pub async fn preload_views(&self, sql_queries: &[&str]) -> Result<()> {
        // Matviews are keyed by hash(sql), so identical queries reuse the same
        // view across restarts. No need to drop — stale views with different
        // queries simply won't be referenced and are harmless.

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

    /// The rendered sort-key spec (`col` / `-col`) implied by the query's
    /// trailing `ORDER BY`, or `None` when it declares no order.
    ///
    /// Only derivable here: the matview body cannot carry the clause (Turso
    /// IVM rejects a Sort node) and the frontend never sees compiled SQL.
    /// Context/parameter binding is irrelevant — an `ORDER BY` term is a
    /// column, never a placeholder.
    pub fn query_ordering_spec(
        &self,
        query: &str,
        language: QueryLanguage,
    ) -> Result<Option<String>> {
        let sql = self.compile_to_sql(query, language)?;
        Ok(crate::sync::trailing_order_by(&sql)
            .as_deref()
            .and_then(crate::sync::order_by_sort_spec))
    }

    /// Compile a PRQL query to raw SQL (no transforms applied).
    fn compile_prql_to_raw_sql(&self, prql: &str) -> Result<String> {
        let full_prql = format!("{}\n{}", PRQL_STDLIB, prql);
        let opts = prqlc::Options::default()
            .with_target(prqlc::Target::Sql(Some(prqlc::sql::Dialect::SQLite)))
            .no_signature();
        let sql = prqlc::compile(&full_prql, &opts)
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
        let schema = self
            .graph_schema_cache
            .read()
            .expect("graph_schema_cache poisoned");
        crate::storage::graph_schema::validate_referenced_edges(&schema, &query)
            .map_err(|e| anyhow::anyhow!("GQL edge validation error: {e}"))?;
        let sql = gql_transform::transform(&query, &schema)
            .map_err(|e| anyhow::anyhow!("GQL transform error: {:?}", e))?;
        Ok(Self::gql_params_to_dollar(&sql))
    }

    /// Convert GQL `:param` syntax to `$param` so `inline_parameters` can read
    /// it. // ALLOW(compatibility): doc describes parameter normalisation, not
    /// a shim
    fn gql_params_to_dollar(sql: &str) -> String {
        use std::fmt::Write;
        let mut result = String::with_capacity(sql.len());
        let mut chars = sql.chars().peekable();
        while let Some(c) = chars.next() {
            if c == ':' {
                if chars
                    .peek()
                    .is_some_and(|ch| ch.is_ascii_alphabetic() || *ch == '_')
                {
                    result.push('$');
                    while chars
                        .peek()
                        .is_some_and(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                    {
                        let _ = write!(result, "{}", chars.next().unwrap());
                    }
                } else {
                    result.push(c);
                }
            } else if c == '\'' {
                // Skip string literals — don't convert inside quoted strings
                result.push(c);
                for sc in chars.by_ref() {
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

    /// Inline parameter values directly into SQL (for materialized view
    /// definitions)
    ///
    /// Unlike bind_parameters which uses `?` placeholders, this function
    /// substitutes actual values into the SQL string. This is necessary for
    /// CREATE MATERIALIZED VIEW statements where the view definition must
    /// contain literal values, not parameters.
    ///
    /// Values are properly escaped/quoted:
    /// - Strings: 'escaped''quotes'
    /// - Numbers: literal
    /// - Null: NULL
    /// - Bool: 1/0
    ///
    /// Shares its placeholder scanner with the execute path's
    /// `bind_parameters`, so the two cannot come to recognize different
    /// placeholder styles for the same query.
    fn inline_parameters(sql: &str, params: &HashMap<String, Value>) -> String {
        rewrite_named_params(sql, &mut |name| params.get(name).map(value_to_sql_literal))
    }

    /// Compute a deterministic view name for a given SQL query and parameters.
    ///
    /// This is used to create materialized views with consistent names,
    /// allowing us to create the view first and then query it for initial
    /// data. Bind context parameters to the parameter map
    ///
    /// Adds `$context_id`, `$context_local_id`, `$context_parent_id`, and
    /// `$context_path_prefix` parameters based on QueryContext. Absent id
    /// values are bound as Value::Null; the path prefix is the context's
    /// PathContext (empty string when `Unfiltered`).
    ///
    /// `$context_local_id` is the same id with its URI scheme stripped
    /// (`cc-session:abc` -> `abc`). Connector mirrors store the entity's own
    /// key scheme-qualified but every foreign key verbatim, so a child
    /// table joins against the local part. Stripping happens HERE, off the
    /// parsed `EntityUri`, so no query has to re-derive it with a `substr`
    /// and a hand-counted offset.
    fn bind_context_params(&self, params: &mut HashMap<String, Value>, context: &QueryContext) {
        match &context.current_block_id {
            Some(id) => {
                params.insert("context_id".into(), Value::String(id.as_str().to_string()));
                params.insert(
                    "context_local_id".into(),
                    Value::String(id.id().to_string()),
                );
            }
            None => {
                params.insert("context_id".into(), Value::Null);
                params.insert("context_local_id".into(), Value::Null);
            }
        }
        match &context.context_parent_id {
            Some(id) => {
                params.insert(
                    "context_parent_id".into(),
                    Value::String(id.as_str().to_string()),
                );
            }
            None => {
                params.insert("context_parent_id".into(), Value::Null);
            }
        }
        // `Unfiltered` binds an empty prefix so `text.starts_with` matches every
        // row; `Under(prefix)` binds the resolved subtree prefix. The former
        // "no prefix" `None` — which bound a zero-row sentinel — is gone: an
        // unresolvable path never reaches here, it is an `Err` at resolution.
        params.insert(
            "context_path_prefix".into(),
            Value::String(context.path_context.prefix_literal().to_string()),
        );
    }

    /// Execute a SQL query and return the result set
    ///
    /// Supports parameter binding by replacing `$param_name` placeholders with
    /// actual values. Parameters are bound safely using SQL parameter
    /// binding to prevent SQL injection.
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
    ) -> Result<Vec<holon_api::StorageEntity>> {
        // Always bind context params (using NULL if no context provided).
        // This enables stdlib virtual tables like `from children` to compile even
        // without context.
        let ctx = context.unwrap_or_else(QueryContext::root);
        self.bind_context_params(&mut params, &ctx);

        // Retry with fresh connections to handle "Database schema changed" errors
        // that occur when DDL operations race with queries during startup.
        // Fresh connections don't have stale prepared statement caches.
        // db_handle used directly
        let mut last_error = None;
        for attempt in 0..5 {
            let result = self.db_handle.query(&sql, params.clone()).await;
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
    /// Note: The SQL should include `_change_origin` column for CDC trace
    /// propagation. When using `compile_query` or `query_and_watch`, this
    /// is handled automatically by the SQL transformers.
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
        self.matview_manager.subscribe_cdc(&view_name).await
    }

    /// Execute a SQL query, set up CDC streaming, and return initial data +
    /// change stream.
    ///
    /// # Arguments
    /// * `sql` - The SQL query to execute and watch
    /// * `params` - Parameters to bind to the query
    /// * `context` - Optional query context for virtual table parameter binding
    ///
    /// # Returns
    /// A `RowChangeStream` where the first batch contains the initial query
    /// results as `Change::Created` items, followed by CDC deltas.
    #[tracing::instrument(skip(self, sql, params, context))]
    pub async fn query_and_watch(
        &self,
        sql: String,
        params: HashMap<String, Value>,
        context: Option<QueryContext>,
    ) -> Result<RowChangeStream> {
        let transformed_sql = self.apply_sql_transforms(&sql);
        tracing::debug!("[BackendEngine] SQL:\n{}", transformed_sql);

        let ctx = context.clone().unwrap_or_else(QueryContext::root);

        // Inline params to get the final SQL for the matview
        let mut params_with_context = params.clone();
        self.bind_context_params(&mut params_with_context, &ctx);
        let sql_with_params = Self::inline_parameters(&transformed_sql, &params_with_context);

        // Shapes the fork's DBSP IVM cannot maintain (subquery predicates —
        // EXISTS / NOT EXISTS / IN) would be served from a silently-empty
        // matview — a fail-loud violation. Route them to eager re-execution on
        // the row-change bus UP FRONT, before any CREATE. The disclosure travels
        // with the stream (batch metadata) so the render surfaces a degraded
        // banner. See `holon_turso::matview_manager::sql_ivm_maintainable`.
        if !holon_turso::matview_manager::sql_ivm_maintainable(&sql_with_params) {
            tracing::warn!(
                sql = %sql_with_params.chars().take(160).collect::<String>(),
                "[query_and_watch] query shape is not IVM-maintainable (subquery predicate); \
                 serving by eager re-execution in disclosed degraded mode"
            );
            return self
                .eager_requery_stream(
                    sql_with_params,
                    "Live results served by re-execution — this query's shape (a subquery \
                     predicate: EXISTS / NOT EXISTS / IN) cannot be incrementally maintained, so \
                     rows are recomputed on each change rather than served from a matview. If a \
                     recompute fails the last known rows are kept until the next change."
                        .to_string(),
                )
                .await;
        }

        // Diagnostic for HANDOFF_DATA_CDC_SCOPE_LEAK.md.
        // HOLON_TRACE_QUERY_BLOCK=<substring> matches against the inlined SQL so
        // we can see exactly which matview is created for a given block_id and
        // correlate with HOLON_TRACE_BLOCK_DATA in the frontend.
        if std::env::var("HOLON_TRACE_VIEWS").is_ok() {
            let view_name_preview =
                crate::sync::MatviewManager::compute_view_name(&sql_with_params);
            tracing::warn!(
                view_name = %view_name_preview,
                sql = %sql_with_params,
                "[diag-cdc-leak] query_and_watch: SQL → view"
            );
        }

        // Ensure view exists, subscribe to CDC, and query initial data.
        // BACKSTOP: the shape predicate above is one classifier; the engine's
        // matview-conversion authority is another. If a shape the predicate
        // thought maintainable is refused PERMANENTLY at CREATE (`Cannot convert
        // LogicalExpr` — e.g. a `CASE` the predicate does not model), fall to
        // eager re-execution and disclose the engine's own refusal text, rather
        // than returning an Err that spins the watcher's retry loop forever on a
        // permanent failure. A TRANSIENT error (a dependency table not built
        // yet) is NOT swallowed — it propagates so the watcher retries.
        let view_name = match self.matview_manager.ensure_view(&sql_with_params).await {
            Ok(view_name) => view_name,
            Err(e) if Self::is_permanent_matview_conversion_error(&e) => {
                tracing::warn!(
                    sql = %sql_with_params.chars().take(160).collect::<String>(),
                    "[query_and_watch] matview CREATE refused permanently ({e}); serving by eager \
                     re-execution in disclosed degraded mode (predicate did not foresee this shape)"
                );
                return self
                    .eager_requery_stream(
                        sql_with_params,
                        format!(
                            "Live results served by re-execution — the engine cannot maintain this \
                             query as a materialized view ({e}). Rows are recomputed on each change."
                        ),
                    )
                    .await;
            }
            Err(e) => return Err(e),
        };
        let cdc_stream = self.matview_manager.subscribe_cdc(&view_name).await?;

        // The snapshot read deliberately does NOT re-apply the ORDER BY that
        // `ensure_view` stripped. Its row order becomes the order of the
        // initial `Created` events on the CDC stream, and that arrival order is
        // load-bearing downstream: re-applying the clause here breaks the left
        // sidebar's nested live_block watch, which then never streams its
        // selectable (keystone `inv` SutFocusWrite::apply_navigate_focus).
        // Honouring a query's declared order is the render layer's job, over a
        // flat collection, not the stream's.
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
                        || err_str.contains("Database schema changed")
                        || err_str.contains("database is locked");
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

        // Fold initial data into the stream as the first batch of Created items.
        // Enrichment (flatten_properties, computed fields) happens downstream in
        // enrich_batch() — same path as CDC updates, so no separate handling needed.
        Ok(Self::prepend_initial_data(data, &view_name, cdc_stream))
    }

    /// Serve a query the IVM engine cannot maintain (see
    /// [`holon_turso::matview_manager::sql_ivm_maintainable`]) by eager
    /// re-execution: read the current rows directly, then re-run the whole
    /// query on every base-table change and emit the diff. No matview is
    /// created, so nothing can go silently stale — the price is O(query) per
    /// change instead of O(delta). Correctness over incrementality; the shape
    /// is rare (planning queries with anti-joins) and disclosed as degraded.
    ///
    /// `disclosure` is the degraded-mode text stamped onto every emitted
    /// batch's metadata; the reactive watcher lifts it onto the query's
    /// `degraded` state so the render discloses the degraded serving mode
    /// (fail-loud: a degraded mode is only acceptable when disclosed).
    async fn eager_requery_stream(
        &self,
        sql_with_params: String,
        disclosure: String,
    ) -> Result<RowChangeStream> {
        let relation = crate::sync::MatviewManager::compute_view_name(&sql_with_params);
        let initial = self
            .db_handle
            .query(&sql_with_params, HashMap::new())
            .await?;
        let cdc_rx = self.db_handle.subscribe_row_changes();
        Ok(Self::spawn_eager_stream(
            self.db_handle.clone(),
            sql_with_params,
            relation,
            disclosure,
            initial,
            cdc_rx,
        ))
    }

    /// A matview CREATE error the engine will NEVER succeed on for THIS SQL —
    /// the IVM compiler refused the shape outright. Retrying cannot help, so
    /// the eager backstop must take over rather than let the watcher spin
    /// forever. The full permanent-refusal class the fork emits (measured,
    /// verifier R3):
    ///   * `Cannot convert LogicalExpr …`           — subquery-predicate
    ///     refusal
    ///   * `… not yet supported …`                  — scalar subquery in
    ///     SELECT, `EXCEPT`/`INTERSECT`
    ///   * `no such column …`                       — derived-table `FROM
    ///     (SELECT…)` (the `_change_origin` transform)
    /// Deliberately NOT matched (TRANSIENT — a dependency not built yet, which
    /// retrying resolves): `no such table`, `waiting for dependencies`,
    /// `database is locked`, `Database schema changed`. The classifier is
    /// biased SAFE anyway: were a transient error mis-classed permanent,
    /// the eager path's own initial query would hit the same error and
    /// propagate it back to the retry loop — self-correcting — whereas the
    /// reverse miss (permanent → transient) is the one that wedges, so we
    /// err toward permanent.
    fn is_permanent_matview_conversion_error(e: &anyhow::Error) -> bool {
        let text = format!("{e:?}");
        if text.contains("no such table") {
            return false; // transient: dependency table not built yet
        }
        text.contains("Cannot convert LogicalExpr")
            || text.contains("not yet supported")
            || text.contains("no such column")
    }

    /// Stable diff key for an eager-served row. An entity row keys on its `id`
    /// (so a `Deleted` matches the reactive layer's id-keyed store). An id-less
    /// aggregate row keys on its sorted `key=value` pairs — DETERMINISTIC
    /// across re-executions, unlike `format!("{row:?}")`, whose `HashMap`
    /// iteration order is per-instance randomized and would mint a fresh
    /// key every tick (full Deleted+Created churn forever).
    fn eager_row_key(row: &StorageEntity) -> String {
        if let Some(Value::String(s)) = row.get("id") {
            return s.to_string();
        }
        let mut pairs: Vec<String> = row.iter().map(|(k, v)| format!("{k}={v:?}")).collect();
        pairs.sort();
        pairs.join("\u{1f}")
    }

    /// The eager re-execution task. Emits the initial rows as `Created`, then
    /// on each base-table CDC batch re-runs `sql`, emitting `Deleted` for rows
    /// that vanished and `Created` for rows that are new or whose content
    /// changed (the reactive layer folds a `Created` for a known key into an
    /// update).
    fn spawn_eager_stream(
        db_handle: DbHandle,
        sql: String,
        relation: String,
        disclosure: String,
        initial: Vec<StorageEntity>,
        mut cdc_rx: broadcast::Receiver<BatchWithMetadata<RowChange>>,
    ) -> RowChangeStream {
        use holon_api::streaming::Batch;
        use holon_api::streaming::BatchMetadata;
        use holon_api::streaming::Change;
        use holon_api::streaming::ChangeOrigin;
        use holon_api::streaming::WithMetadata;

        let (tx, rx) = tokio::sync::mpsc::channel(1024);

        let make_batch = |items: Vec<RowChange>, relation: &str, seq: u64, disclosure: &str| {
            WithMetadata {
                inner: Batch { items },
                metadata: BatchMetadata {
                    relation_name: relation.to_string(),
                    trace_context: None,
                    linked_contexts: Vec::new(),
                    sync_token: None,
                    seq,
                    // Every eager batch carries the disclosure so a watcher that
                    // subscribes mid-stream still learns it is degraded.
                    degraded: Some(disclosure.to_string()),
                },
            }
        };
        let created = |row: StorageEntity, relation: &str| RowChange {
            relation_name: relation.to_string(),
            change: Change::Created {
                data: row,
                origin: ChangeOrigin::Local {
                    operation_id: None,
                    trace_id: None,
                },
            },
        };

        crate::util::spawn_actor(async move {
            let mut prev: HashMap<String, StorageEntity> = HashMap::new();
            let mut seq: u64 = 0;

            let initial_changes: Vec<RowChange> = initial
                .into_iter()
                .map(|row| {
                    prev.insert(Self::eager_row_key(&row), row.clone());
                    created(row, &relation)
                })
                .collect();
            if tx
                .send(make_batch(initial_changes, &relation, seq, &disclosure))
                .await
                .is_err()
            {
                return;
            }

            loop {
                match cdc_rx.recv().await {
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return,
                }
                // Coalesce a burst of changes into one re-execution.
                while cdc_rx.try_recv().is_ok() {}

                let rows = match db_handle.query(&sql, HashMap::new()).await {
                    Ok(rows) => rows,
                    Err(e) => {
                        // NOT silent: the whole query is already served under a
                        // standing degraded-mode disclosure (the frontend sets it
                        // for every eager-served shape, and its text says the last
                        // known rows are retained if a refresh fails). Keeping the
                        // last-good rows here — rather than blanking to empty — is
                        // the disclosed behaviour; the failure is surfaced at ERROR
                        // for debuggability and the next CDC tick retries. The
                        // INITIAL load failure is loud separately: it propagates as
                        // `Err` from `eager_requery_stream` (before this loop) into
                        // the frontend watcher's `set_error`.
                        tracing::error!(
                            relation = %relation,
                            "[eager_requery] re-execution failed; retaining last-known rows under \
                             the standing degraded disclosure, will retry on next change: {e}"
                        );
                        continue;
                    }
                };
                let next: HashMap<String, StorageEntity> = rows
                    .into_iter()
                    .map(|row| (Self::eager_row_key(&row), row))
                    .collect();

                let mut items: Vec<RowChange> = Vec::new();
                for (key, _) in prev.iter() {
                    if !next.contains_key(key) {
                        items.push(RowChange {
                            relation_name: relation.clone(),
                            change: Change::Deleted {
                                id: key.clone(),
                                origin: ChangeOrigin::Local {
                                    operation_id: None,
                                    trace_id: None,
                                },
                            },
                        });
                    }
                }
                for (key, row) in next.iter() {
                    if prev.get(key) != Some(row) {
                        items.push(created(row.clone(), &relation));
                    }
                }

                prev = next;
                if items.is_empty() {
                    continue;
                }
                seq += 1;
                if tx
                    .send(make_batch(items, &relation, seq, &disclosure))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        });

        tokio_stream::wrappers::ReceiverStream::new(rx)
    }

    /// Create a RowChangeStream that emits initial rows as the first `Created`
    /// batch, then forwards CDC updates from the underlying stream.
    fn prepend_initial_data(
        initial_rows: Vec<holon_core::storage::types::StorageEntity>,
        view_name: &str,
        mut cdc_stream: RowChangeStream,
    ) -> RowChangeStream {
        use holon_api::streaming::Batch;
        use holon_api::streaming::BatchMetadata;
        use holon_api::streaming::Change;
        use holon_api::streaming::WithMetadata;
        use tokio_stream::StreamExt;

        let view_name = view_name.to_string();
        let (tx, rx) = tokio::sync::mpsc::channel(1024);

        crate::util::spawn_actor(async move {
            // Emit initial rows as Created changes
            let initial_changes: Vec<RowChange> = initial_rows
                .into_iter()
                .map(|row| RowChange {
                    relation_name: view_name.clone(),
                    change: Change::Created {
                        data: row,
                        origin: holon_api::streaming::ChangeOrigin::Local {
                            operation_id: None,
                            trace_id: None,
                        },
                    },
                })
                .collect();
            let initial_batch = WithMetadata {
                inner: Batch {
                    items: initial_changes,
                },
                metadata: BatchMetadata {
                    relation_name: view_name.clone(),
                    trace_context: None,
                    linked_contexts: Vec::new(),
                    sync_token: None,
                    seq: 0,
                    degraded: None,
                },
            };
            if tx.send(initial_batch).await.is_err() {
                return;
            }

            // Forward CDC stream
            while let Some(batch) = cdc_stream.next().await {
                if tx.send(batch).await.is_err() {
                    break;
                }
            }
        });

        tokio_stream::wrappers::ReceiverStream::new(rx)
    }

    /// Execute a block operation
    ///
    /// This method provides a clean interface for executing operations without
    /// exposing the internal TursoBackend. It handles locking and passes
    /// the current UI state.
    ///
    /// # Arguments
    /// * `op_name` - Name of the operation to execute (e.g., "indent",
    ///   "outdent", "move_block")
    /// * `params` - Parameters for the operation (typically includes block ID
    ///   and operation-specific fields)
    ///
    /// # Returns
    /// Result indicating success or failure. On success, UI should re-query to
    /// get updated data.
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
    /// params.insert("id".into(), Value::String("block-1".to_string()));
    ///
    /// engine.execute_operation("indent", params).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
        origin: holon_api::OpOrigin,
    ) -> Result<holon_api::OpOutcome> {
        use tracing::Instrument;
        use tracing::info;

        // Create tracing span that will be bridged to OpenTelemetry
        // Use .instrument() to maintain context across async boundaries
        let span = tracing::span!(
            tracing::Level::INFO,
            "backend.execute_operation",
            "operation.entity" = entity_name.to_string(),
            "operation.name" = op_name,
            "operation.origin" = origin.tag()
        );

        async {
            info!(
                "[BackendEngine] execute_operation: entity={}, op={}, origin={}, params={:?}",
                entity_name,
                op_name,
                origin.tag(),
                params
            );

            // Dispatch + undo-stack bookkeeping live in the shared op engine
            // (over the same dispatcher). Span context propagates via the
            // tracing-opentelemetry bridge.
            self.op_engine
                .execute_operation(entity_name, op_name, params, origin)
                .await
        }
        .instrument(span)
        .await
    }

    /// Replace the in-memory undo engine with a persistent one backed by the
    /// replica DB: the `undo_log` snapshot table plus a live-state reader for
    /// precondition (staleness) verification. Called once during DI init while
    /// the engine is still owned (before it is shared behind `Arc`).
    pub async fn enable_undo_persistence(&mut self) -> Result<()> {
        use crate::api::undo_persistence::SqlUndoStateReader;
        use crate::api::undo_persistence::SqlUndoStore;
        use crate::api::undo_persistence::ensure_undo_log;
        ensure_undo_log(&self.db_handle).await?;
        let reader = Arc::new(SqlUndoStateReader::new(
            self.db_handle.clone(),
            crate::storage::BLOCK_WRITE_TABLE,
        ));
        let store = Arc::new(SqlUndoStore::new(self.db_handle.clone()));
        let history = Arc::new(crate::api::history_store::TursoHistoryStore::new(
            self.db_handle.clone(),
        ));
        self.op_engine = crate::api::operation_engine::DispatchingOperationEngine::new_persistent(
            self.dispatcher.clone(),
            reader,
            store,
        )
        .await?
        .with_history_store(history)
        .with_template_source(Arc::new(
            crate::api::template_source::TursoTemplateSource::new(self.db_handle.clone()),
        ))
        .with_task_vocabulary_source(Arc::new(
            crate::api::task_vocabulary_source::SqlTaskVocabularySource::new(
                self.db_handle.clone(),
                crate::storage::BLOCK_WRITE_TABLE,
            ),
        ));
        Ok(())
    }

    /// Follow `id` through the merge redirects to the identity that currently
    /// holds it. An id nobody merged away resolves to itself, so every caller
    /// can route through this unconditionally.
    ///
    /// This is the ONE resolution seam: a lookup that misses consults
    /// `block_redirects` here rather than each reader re-implementing the
    /// chain walk. Fails loud on a cycle instead of spinning — `merge_blocks`
    /// refuses to create one, so reaching it means the table was corrupted.
    pub async fn resolve_block_id(
        &self,
        id: &holon_api::EntityUri,
    ) -> Result<holon_api::EntityUri> {
        let mut current = id.to_string();
        let mut chain = vec![current.clone()];
        loop {
            let mut params = std::collections::HashMap::new();
            params.insert("from_id".to_string(), Value::String(current.clone()));
            let rows = self
                .db_handle
                .query(
                    "SELECT to_id FROM block_redirects WHERE from_id = $from_id",
                    params,
                )
                .await
                .map_err(|e| anyhow::anyhow!("resolve_block_id({id}): {e}"))?;
            let Some(next) = rows
                .first()
                .and_then(|r| r.get("to_id"))
                .and_then(|v| v.as_string())
                .map(str::to_string)
            else {
                break;
            };
            if chain.contains(&next) {
                anyhow::bail!(
                    "block_redirects holds a cycle reached from {id}: {} -> {next}",
                    chain.join(" -> ")
                );
            }
            chain.push(next.clone());
            current = next;
        }

        // A redirect whose terminal no longer exists (the survivor was deleted
        // after the merge) must NOT come back as if it named a live block —
        // that is the silent-wrong-answer case. Disclose the whole chain so the
        // stranding is obvious. Only checked when a redirect was actually
        // followed; an unmerged id missing is the caller's own lookup to miss.
        if chain.len() > 1 {
            let mut params = std::collections::HashMap::new();
            params.insert("id".to_string(), Value::String(current.clone()));
            let rows = self
                .db_handle
                .query("SELECT id FROM block_raw WHERE id = $id", params)
                .await
                .map_err(|e| anyhow::anyhow!("resolve_block_id({id}) terminal check: {e}"))?;
            if rows.is_empty() {
                anyhow::bail!(
                    "merge redirect {} ends at '{current}', which no longer exists — the merge \
                     survivor was deleted, stranding every id merged into it",
                    chain.join(" -> ")
                );
            }
        }
        // ALLOW(entity_uri_from_raw): id read back from a block_redirects row
        Ok(holon_api::EntityUri::from_raw(&current))
    }

    /// Undo the last operation.
    ///
    /// Delegates to the shared op engine. Returns true if an operation was
    /// undone, false if the undo stack is empty.
    pub async fn undo(&self) -> Result<holon_api::UndoOutcome> {
        self.op_engine.undo().await
    }

    /// Redo the last undone operation.
    ///
    /// Delegates to the shared op engine. Returns true if an operation was
    /// redone, false if the redo stack is empty.
    pub async fn redo(&self) -> Result<holon_api::UndoOutcome> {
        self.op_engine.redo().await
    }

    /// Check if undo is available
    pub async fn can_undo(&self) -> bool {
        self.op_engine.can_undo().await
    }

    /// Check if redo is available
    pub async fn can_redo(&self) -> bool {
        self.op_engine.can_redo().await
    }

    /// Open a composite-undo group (Inc1): buffer subsequent User-origin ops
    /// into ONE undo entry until [`end_undo_group`](Self::end_undo_group).
    /// Delegates to the shared op engine. See
    /// [`DispatchingOperationEngine::begin_undo_group`].
    pub async fn begin_undo_group(&self) {
        self.op_engine.begin_undo_group().await
    }

    /// Close the innermost composite-undo group, materializing the buffered
    /// sub-ops into one composite entry. Delegates to the shared op engine.
    pub async fn end_undo_group(&self) -> Result<()> {
        self.op_engine.end_undo_group().await
    }

    /// Test-only: push a hand-crafted [`holon_core::UndoEntry`] onto the shared
    /// engine's stack to exercise composite-inverse replay paths.
    #[cfg(test)]
    pub(crate) async fn push_undo_entry_for_test(&self, entry: holon_core::UndoEntry) {
        self.op_engine.push_undo_entry_for_test(entry).await;
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
    /// use holon_core::OperationProvider;
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
        self.op_engine.available_operations(entity_name).await
    }

    pub async fn has_operation(&self, entity_name: &str, op_name: &str) -> bool {
        self.op_engine.has_operation(entity_name, op_name).await
    }

    /// Every operation descriptor this engine can dispatch — the descriptor
    /// half of the ADR 0032 net's sources.
    ///
    /// Two sources, because the engine has two: the dispatcher's registered
    /// providers, plus the engine-synthetic `block` compounds, which are not
    /// providers and so appear in neither `operations()` nor `all_providers()`.
    ///
    /// Both sync fan-out layers are EXCLUDED. The wildcard `*::sync` /
    /// `*::full_sync` re-dispatch to each syncable provider, and each of those
    /// lands on a `<provider>.sync` descriptor which re-dispatches into the
    /// provider's own entity ops. Neither layer names a relation, so neither
    /// has a place to lower an arc onto. `holon_core::classify_for_net` owns
    /// that judgement and refuses — loudly, as an `Err` this propagates — any
    /// descriptor that claims the shape without earning it, so a future
    /// fan-out-named op that actually writes cannot slide out of the net.
    ///
    /// Duplicates keep the FIRST occurrence, which is the routing rule dispatch
    /// itself follows (`execute_operation` takes the first provider advertising
    /// the pair), so the net describes the descriptor that would actually run.
    /// Duplicates are policed where they arise, by the registry-uniqueness
    /// assertion in `OperationDispatcher::operations`.
    pub fn operation_catalog(&self) -> Result<Vec<OperationDescriptor>> {
        use holon_core::OperationProvider as _;

        let mut catalog = Vec::new();
        let mut claimed = std::collections::HashSet::new();
        let synthetic = self.op_engine.firable_block_synthetic_descriptors();
        for descriptor in self.dispatcher.operations().into_iter().chain(synthetic) {
            let admission = holon_core::classify_for_net(&descriptor)
                .map_err(|e| anyhow::anyhow!("the net's descriptor catalog refuses one: {e}"))?;
            if admission == holon_core::NetAdmission::FanOutMarker {
                continue;
            }
            if claimed.insert((descriptor.entity_name.clone(), descriptor.name.clone())) {
                catalog.push(descriptor);
            }
        }
        Ok(catalog)
    }

    /// The ADR 0032 derived net for this engine's CURRENT sources.
    ///
    /// Recomputed on every call and held nowhere. Providers register after boot
    /// (`declare_type`) and rule blocks are discovered reactively, so a
    /// snapshot cached beside this call would describe a world the engine
    /// has already left — do not add one "for symmetry" with the other
    /// derived artifacts.
    ///
    /// A reactive var recomputed on source change is a different thing and is
    /// permitted (ADR 0032 §2): it re-derives on the change rather than
    /// persisting a snapshot, so it has no staleness window to invalidate.
    pub fn derived_net(&self) -> Result<holon_net::CompiledNet> {
        let descriptors = self.operation_catalog()?;
        let rules = self.accepted_rules.sources();
        Ok(holon_net::derive_net(&descriptors, &rules)?)
    }

    /// Map a table name to an entity name
    ///
    /// This mapping is used during query compilation to determine which
    /// entity type operations are available for a given table.
    ///
    /// # Arguments
    /// * `table_name` - Database table name (e.g., "todoist_task",
    ///   "logseq_block")
    /// * `entity_name` - Entity identifier (e.g., "todoist-task",
    ///   "logseq-block")
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
    /// This allows querying available operations without mutating the
    /// dispatcher.
    pub fn get_dispatcher(&self) -> Arc<OperationDispatcher> {
        self.dispatcher.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use holon_api::EntityUri;

    use super::*;
    use crate::core::sql_operation_provider::SqlOperationProvider;
    use crate::di::test_helpers::create_test_engine;
    use crate::di::test_helpers::create_test_engine_with_providers;

    #[test]
    fn prql_stdlib_compiles_successfully() {
        let full_prql = format!("{}\nfrom block", PRQL_STDLIB);
        let opts = prqlc::Options::default()
            .with_target(prqlc::Target::Sql(Some(prqlc::sql::Dialect::SQLite)))
            .no_signature();
        prqlc::compile(&full_prql, &opts).expect("PRQL_STDLIB should compile without errors");
    }

    #[test]
    fn test_inline_parameters() {
        let mut params = HashMap::new();
        params.insert("context_id".into(), Value::String("block-123".to_string()));
        params.insert("context_parent_id".into(), Value::Null);
        params.insert("num".into(), Value::Integer(42));
        params.insert("flag".into(), Value::Boolean(true));

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
        let result = create_test_engine().await;
        assert!(result.is_ok());
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

    /// The frontend never sees compiled SQL, so a `sort {-x}` sidecar's order
    /// can only reach the rendered collection through this derivation.
    #[tokio::test(flavor = "multi_thread")]
    async fn ordering_spec_carries_a_prql_sort_to_the_render_layer() {
        let engine = create_test_engine().await.unwrap();

        assert_eq!(
            engine
                .query_ordering_spec(
                    "from block | select {id, content} | sort {-content}",
                    QueryLanguage::HolonPrql
                )
                .expect("PRQL compile")
                .as_deref(),
            Some("-content")
        );
        assert_eq!(
            engine
                .query_ordering_spec(
                    "from block | select {id, content} | sort content",
                    QueryLanguage::HolonPrql
                )
                .expect("PRQL compile")
                .as_deref(),
            Some("content")
        );
        assert_eq!(
            engine
                .query_ordering_spec(
                    "from block | select {id, content}",
                    QueryLanguage::HolonPrql
                )
                .expect("PRQL compile"),
            None,
            "a query that declares no order must not impose one"
        );
    }

    /// Validates the H1 hypothesis from HANDOFF_DATA_CDC_SCOPE_LEAK.md:
    /// `from children` PRQL must produce SQL with `parent_id = 'block:test-1'`
    /// after `bind_context_params` + `inline_parameters`.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_from_children_substitutes_block_id() {
        let engine = create_test_engine().await.unwrap();
        let raw_sql = engine
            .compile_to_sql("from children", QueryLanguage::HolonPrql)
            .expect("PRQL compile");

        let context = QueryContext::for_block_with_path(
            &EntityUri::block("test-1"),
            None,
            "/test-1".to_string(),
        );
        let mut params = HashMap::new();
        engine.bind_context_params(&mut params, &context);
        let inlined = BackendEngine::inline_parameters(&raw_sql, &params);

        assert!(
            inlined.contains("'block:test-1'"),
            "expected substituted block id literal in SQL, got:\n{inlined}"
        );
        assert!(
            !inlined.contains("$context_id"),
            "context_id should be substituted, got:\n{inlined}"
        );
        assert!(
            inlined.to_lowercase().contains("parent_id"),
            "expected parent_id predicate, got:\n{inlined}"
        );
    }

    /// `$context_local_id` is the same id with its scheme stripped, so a
    /// connector's child table (whose foreign keys are the provider's raw ids)
    /// can join without a hand-counted `substr` offset.
    #[tokio::test(flavor = "multi_thread")]
    async fn context_local_id_is_the_context_id_without_its_scheme() {
        let engine = create_test_engine().await.unwrap();
        let context = QueryContext::for_block(
            &EntityUri::parse("cc-session:5969a71e").expect("valid entity URI"),
            None,
        );
        let mut params = HashMap::new();
        engine.bind_context_params(&mut params, &context);

        let inlined = BackendEngine::inline_parameters(
            "SELECT 1 FROM cc_message WHERE session_id = $context_local_id AND owner = \
             $context_id",
            &params,
        );
        assert_eq!(
            inlined,
            "SELECT 1 FROM cc_message WHERE session_id = '5969a71e' AND owner = \
             'cc-session:5969a71e'"
        );
    }

    /// End-to-end check that `from children` only returns the parent's direct
    /// A focused editor decides whether an external row change is newer than
    /// its own last keystroke from the `write_seq` its row carries
    /// (`holon_frontend::echo::evaluate_data_sync_echo`). A stdlib source whose
    /// `select` drops the column hands that decision a `None` and the change is
    /// discarded — so every block-shaped virtual table must project it.
    #[tokio::test(flavor = "multi_thread")]
    async fn every_stdlib_block_source_projects_the_editors_ordering_token() {
        let engine = create_test_engine().await.unwrap();
        engine
            .db_handle()
            .execute(
                &format!(
                    "INSERT INTO {table} (id, parent_id, content, content_type) VALUES \
                     ('block:p', 'sentinel:no_parent', 'Parent', 'text'), ('block:p::child::0', \
                     'block:p', 'Child A', 'text')",
                    table = crate::storage::BLOCK_WRITE_TABLE,
                ),
                vec![],
            )
            .await
            .unwrap();
        // `focused_children` reaches its rows through the main region's open
        // navigation cursor.
        engine
            .db_handle()
            .execute(
                // Plain INSERT, not INSERT OR REPLACE: `navigation_history` is a
                // rowid-alias table (`id INTEGER PRIMARY KEY AUTOINCREMENT`) and
                // the base of the `focus_roots` / `current_focus` matviews, which
                // is the one shape the fork's IVM corrupts on a REPLACE — so the
                // DbHandle guard refuses it. The row is fresh here anyway.
                "INSERT INTO navigation_history (id, region, block_id) VALUES (1, 'main', \
                 'block:p')",
                vec![],
            )
            .await
            .unwrap();
        engine
            .db_handle()
            .execute(
                "INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 1)",
                vec![],
            )
            .await
            .unwrap();

        let context = QueryContext {
            current_block_id: Some(EntityUri::block("p")),
            context_parent_id: Some(EntityUri::block("p")),
            path_context: holon_api::PathContext::Unfiltered,
        };

        for source in [
            "children",
            "siblings",
            "descendants",
            "block_children",
            "focused_children",
        ] {
            let sql = engine
                .compile_to_sql(&format!("from {source}"), QueryLanguage::HolonPrql)
                .expect("PRQL compile");
            let rows = engine
                .execute_query(sql, HashMap::new(), Some(context.clone()))
                .await
                .unwrap_or_else(|e| panic!("`from {source}` query failed: {e}"));
            assert!(
                !rows.is_empty(),
                "`from {source}` returned no rows, so the projection assertion below would be \
                 vacuous"
            );
            for row in &rows {
                assert!(
                    row.get(holon_api::schema::block::WRITE_SEQ).is_some(),
                    "`from {source}` dropped the {} column; the focused editor reads it as None \
                     and discards the external change. Row: {row:?}",
                    holon_api::schema::block::WRITE_SEQ,
                );
            }
        }
    }

    /// (non-source) children — exercises compile → bind → matview create →
    /// query_view, the same path `render_entity` runs for query blocks.
    /// Regression for HANDOFF_DATA_CDC_SCOPE_LEAK.md.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_from_children_matview_returns_only_children() {
        use tokio_stream::StreamExt;
        let engine = create_test_engine().await.unwrap();

        // Seed: parent + two children of parent + an unrelated block at root
        // (parent_id = NULL) + an unrelated block under a different parent.
        engine
            .db_handle()
            .execute(
                &format!(
                    "INSERT INTO {table} (id, parent_id, content, content_type) VALUES \
                     ('block:p', NULL, 'Parent', 'text'), ('block:p::child::0', 'block:p', 'Child \
                     A', 'text'), ('block:p::child::1', 'block:p', 'Child B', 'text'), \
                     ('block:p::src::0', 'block:p', 'from children', 'source'), ('block:other', \
                     NULL, 'Unrelated Root', 'text'), ('block:other::child::0', 'block:other', \
                     'Unrelated Child', 'text')",
                    table = crate::storage::BLOCK_WRITE_TABLE,
                ),
                vec![],
            )
            .await
            .unwrap();

        let sql = engine
            .compile_to_sql("from children", QueryLanguage::HolonPrql)
            .expect("PRQL compile");
        let context =
            QueryContext::for_block_with_path(&EntityUri::block("p"), None, "/p".to_string());

        let stream = engine
            .query_and_watch(sql, HashMap::new(), Some(context))
            .await
            .expect("query_and_watch");

        tokio::pin!(stream);
        let initial = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .expect("initial batch within 5s")
            .expect("stream should not close");

        let mut row_ids: Vec<String> = initial
            .inner
            .items
            .iter()
            .filter_map(|c| match &c.change {
                holon_api::streaming::Change::Created { data, .. } => data
                    .get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string()),
                _ => None,
            })
            .collect();
        row_ids.sort();

        // PRQL stdlib: `from children = from block | filter parent_id == $context_id
        //                                       | filter content_type != 'source'`.
        // Source-typed `block:p::src::0` is filtered out; `block:other*` rows are
        // not children of `block:p`.
        assert_eq!(
            row_ids,
            vec![
                "block:p::child::0".to_string(),
                "block:p::child::1".to_string(),
            ],
            "from-children matview leaked rows; full set: {row_ids:?}"
        );
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
        params.insert("min_depth".into(), Value::Integer(0));

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
                "INSERT INTO users VALUES ('u1', 'Alice', 30), ('u2', 'Bob', 25), ('u3', \
                 'Charlie', 35)",
                vec![],
            )
            .await
            .unwrap();

        // Test multiple parameters
        let mut params = HashMap::new();
        params.insert("min_age".into(), Value::Integer(25));
        params.insert("max_age".into(), Value::Integer(35));

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
                // entity names normalize `_` -> `-` (EntityName::new, URI-scheme
                // safety); only the SQL table keeps the underscore.
                Arc::new(SqlOperationProvider::new(
                    db_handle,
                    "test_item".to_string(),
                    "test-item".to_string(),
                    "test-item".to_string(),
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
        let mut params: StorageEntity = holon_api::StorageEntity::new();
        params.insert("id".into(), Value::String("item-1".to_string()));
        params.insert("field".into(), Value::String("completed".to_string()));
        params.insert("value".into(), Value::Boolean(true));

        let result = engine
            .execute_operation(
                &EntityName::new("test_item"),
                "set_field",
                params,
                holon_api::OpOrigin::User,
            )
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
            .execute_operation(
                &EntityName::Named("block".to_string()),
                "nonexistent",
                params,
                holon_api::OpOrigin::User,
            )
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

    /// Ruling C / #41: a root (no-filter) context must bind an UNFILTERED path
    /// predicate — an empty prefix that `text.starts_with` matches on every row
    /// — NEVER the `__NO_PATH__/` sentinel that silently matched zero rows (the
    /// six-round nested-page chevron class, #27). The sentinel must be
    /// unrepresentable after the PathContext split.
    #[tokio::test(flavor = "multi_thread")]
    async fn root_context_binds_unfiltered_path_prefix_not_sentinel() {
        let engine = create_test_engine().await.unwrap();
        let mut params = HashMap::new();
        engine.bind_context_params(&mut params, &QueryContext::root());
        assert_eq!(
            params.get("context_path_prefix"),
            Some(&Value::String(String::new())),
            "root/unfiltered context must bind an empty prefix (matches every row)"
        );
        for v in params.values() {
            if let Value::String(s) = v {
                assert!(
                    !s.contains("__NO_PATH__"),
                    "the __NO_PATH__ sentinel leaked into a bound param: {s:?}"
                );
            }
        }
    }

    /// Ruling C / #41: the actual `from descendants` SQL, bound under a root
    /// (unfiltered) context, must carry NO `__NO_PATH__/` sentinel. The
    /// sentinel made the path predicate match zero rows (silent-empty); an
    /// empty prefix makes `text.starts_with` match every row instead. This
    /// is the compile- seam enforcement the chevron hunt lacked
    /// (`block_with_path` is not populated in the unit test engine, so this
    /// asserts on the bound query text rather than a live matview row set).
    #[tokio::test(flavor = "multi_thread")]
    async fn descendants_under_root_binds_no_sentinel_predicate() {
        let engine = create_test_engine().await.unwrap();
        let raw_sql = engine
            .compile_to_sql("from descendants", QueryLanguage::HolonPrql)
            .expect("PRQL compile");
        let mut params = HashMap::new();
        engine.bind_context_params(&mut params, &QueryContext::root());
        let inlined = BackendEngine::inline_parameters(&raw_sql, &params);

        assert!(
            !inlined.contains("__NO_PATH__"),
            "root descendants query must not carry the silent-empty sentinel, got:\n{inlined}"
        );
        let lowered = inlined.to_lowercase();
        assert!(
            lowered.contains("path") && lowered.contains("like"),
            "expected a `path LIKE …` descendants predicate, got:\n{inlined}"
        );
    }

    /// #45: a missing block's path lookup must FAIL LOUD, never fabricate
    /// `/{id}`. A fabricated path silently mis-scopes descendants queries onto
    /// a path that no other block shares.
    #[tokio::test(flavor = "multi_thread")]
    async fn lookup_block_path_errs_on_missing_block_not_fabricated() {
        let engine = create_test_engine().await.unwrap();
        let missing = EntityUri::block("does-not-exist-9f3a");
        let result = engine.blocks().lookup_block_path(&missing).await;
        assert!(
            result.is_err(),
            "missing-block path lookup must Err (fail loud), got fabricated: {result:?}"
        );
    }

    /// Render-unblock for the Now.org anti-join bug
    /// (bugfunnel 2026-08-19-ivm-antijoin-matview-silently-empty): a live_query
    /// whose shape the IVM engine cannot maintain must be served CORRECTLY by
    /// eager re-execution, not from a silently-empty matview. The matview path
    /// returns 0 rows here; the eager path must return the 5 unblocked TODOs
    /// AND react to a mutation.
    #[tokio::test(flavor = "multi_thread")]
    async fn antijoin_live_query_served_eagerly_yields_rows_and_reacts() {
        use tokio_stream::StreamExt;

        let engine = create_test_engine().await.unwrap();
        let table = crate::storage::BLOCK_WRITE_TABLE;

        // Five unblocked, agent-tagged, G1 TODOs (b0..b4) + a DONE dep (b8) that
        // does NOT block b0.
        for i in 0..5 {
            engine
                .db_handle()
                .execute(
                    &format!(
                        "INSERT INTO {table} (id, parent_id, content, content_type, properties) \
                         VALUES ('block:b{i}', 'sentinel:no_parent', 'T{i}', 'text', \
                         '{{\"task_state\":\"TODO\",\"gate\":\"G1\"}}')"
                    ),
                    vec![],
                )
                .await
                .unwrap();
            engine
                .db_handle()
                .execute(
                    &format!(
                        "INSERT INTO block_tags (block_id, tag) VALUES ('block:b{i}', 'agent')"
                    ),
                    vec![],
                )
                .await
                .unwrap();
        }
        engine
            .db_handle()
            .execute(
                &format!(
                    "INSERT INTO {table} (id, parent_id, content, content_type, properties) VALUES \
                     ('block:b8', 'sentinel:no_parent', 'Done dep', 'text', \
                     '{{\"task_state\":\"DONE\",\"gate\":\"G2\"}}')"
                ),
                vec![],
            )
            .await
            .unwrap();
        engine
            .db_handle()
            .execute(
                "INSERT INTO block_requires (block_id, required_id) VALUES ('block:b0', 'block:b8')",
                vec![],
            )
            .await
            .unwrap();

        let sql = "SELECT b.* FROM block b WHERE \
            json_extract(b.properties,'$.task_state') = 'TODO' AND \
            json_extract(b.properties,'$.gate') = 'G1' AND \
            NOT EXISTS (SELECT 1 FROM block_requires br JOIN block bl ON bl.id = br.required_id \
                WHERE br.block_id = b.id AND COALESCE(json_extract(bl.properties,'$.task_state'),'') != 'DONE') AND \
            (EXISTS (SELECT 1 FROM block_tags bt WHERE bt.block_id = b.id AND bt.tag = 'agent') \
             OR NOT EXISTS (SELECT 1 FROM block_tags bt WHERE bt.block_id = b.id AND bt.tag = 'human-only'))";

        let mut stream = engine
            .query_and_watch(sql.to_string(), HashMap::new(), None)
            .await
            .expect("watch anti-join query");

        // First batch is the eager initial snapshot: the 5 unblocked TODOs.
        let first = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .expect("initial batch within 5s")
            .expect("stream yields an initial batch");
        let created = first
            .inner
            .items
            .iter()
            .filter(|c| matches!(c.change, holon_api::streaming::Change::Created { .. }))
            .count();
        assert_eq!(
            created, 5,
            "eager initial snapshot must serve the 5 unblocked TODOs (matview serves 0)"
        );
        // The disclosure travels with the stream so the render discloses the
        // degraded serving mode.
        assert!(
            first
                .metadata
                .degraded
                .as_deref()
                .is_some_and(|d| d.contains("re-execution")),
            "eager batch must carry the degraded disclosure; got {:?}",
            first.metadata.degraded
        );

        // React to a mutation: block b1 by requiring an unfinished task -> the
        // eager re-execution must emit a Deleted for b1.
        engine
            .db_handle()
            .execute(
                &format!(
                    "INSERT INTO {table} (id, parent_id, content, content_type, properties) VALUES \
                     ('block:b7', 'sentinel:no_parent', 'Open dep', 'text', \
                     '{{\"task_state\":\"TODO\",\"gate\":\"G2\"}}')"
                ),
                vec![],
            )
            .await
            .unwrap();
        engine
            .db_handle()
            .execute(
                "INSERT INTO block_requires (block_id, required_id) VALUES ('block:b1', 'block:b7')",
                vec![],
            )
            .await
            .unwrap();

        let mut saw_delete_b1 = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_secs(1), stream.next()).await {
                Ok(Some(batch)) => {
                    for change in &batch.inner.items {
                        if let holon_api::streaming::Change::Deleted { id, .. } = &change.change
                            && id.contains("b1")
                        {
                            saw_delete_b1 = true;
                        }
                    }
                    if saw_delete_b1 {
                        break;
                    }
                }
                _ => break,
            }
        }
        assert!(
            saw_delete_b1,
            "eager re-execution must retract b1 once it becomes blocked"
        );
    }

    /// The eager BACKSTOP: a shape the routing predicate thinks is maintainable
    /// (no subquery node) but the engine REFUSES PERMANENTLY at matview CREATE
    /// (`CASE` → "Cannot convert LogicalExpr") must still be served by eager
    /// re-execution WITH a disclosure — never a wedged/error widget. Two
    /// classifiers (parser + engine), defense-in-depth.
    #[tokio::test(flavor = "multi_thread")]
    async fn permanent_matview_refusal_falls_back_to_eager_with_disclosure() {
        use tokio_stream::StreamExt;

        let engine = create_test_engine().await.unwrap();
        let table = crate::storage::BLOCK_WRITE_TABLE;
        engine
            .db_handle()
            .execute(
                &format!(
                    "INSERT INTO {table} (id, parent_id, content, content_type, properties) VALUES \
                     ('block:c0', 'sentinel:no_parent', 'C0', 'text', '{{\"task_state\":\"TODO\"}}')"
                ),
                vec![],
            )
            .await
            .unwrap();

        // `CASE` has no Exists/InSubquery node, so `sql_ivm_maintainable` returns
        // true and the matview path is taken — where the fork's IVM planner
        // refuses `CASE` at DDL. The backstop must catch that and serve eager.
        assert!(
            holon_turso::matview_manager::sql_ivm_maintainable(
                "SELECT b.id, CASE WHEN json_extract(b.properties,'$.task_state')='TODO' THEN 1 \
                 ELSE 0 END AS flag FROM block b"
            ),
            "sanity: the predicate does NOT flag CASE (only the engine refuses it) — that is why \
             the backstop exists"
        );

        let sql = "SELECT b.id, CASE WHEN json_extract(b.properties,'$.task_state')='TODO' THEN 1 \
                   ELSE 0 END AS flag FROM block b";
        let mut stream = engine
            .query_and_watch(sql.to_string(), HashMap::new(), None)
            .await
            .expect("watch_query must NOT error on a permanent CREATE refusal — it serves eager");

        let first = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .expect("initial batch within 5s")
            .expect("stream yields an initial batch");
        let created = first
            .inner
            .items
            .iter()
            .filter(|c| matches!(c.change, holon_api::streaming::Change::Created { .. }))
            .count();
        assert_eq!(
            created, 1,
            "eager backstop must serve the row the matview could not"
        );
        assert!(
            first
                .metadata
                .degraded
                .as_deref()
                .is_some_and(|d| d.contains("materialized view") && d.contains("Cannot convert")),
            "backstop disclosure must carry the engine's own refusal text; got {:?}",
            first.metadata.degraded
        );
    }

    /// Fork-regression GUARD for the `LEFT JOIN … IS NULL` anti-join. The
    /// routing predicate keeps this shape on the matview path (no `Exists`/
    /// `InSubquery` node), so it is served by the IVM matview — which the turso
    /// populate fix (`c6cfab7d`, "matview-antijoin-populate-fix") makes
    /// CORRECT. On the pre-fix pin `54f3cc5e` this over-served (direct=4,
    /// matview=5, undisclosed); measured 4==4 on `c6cfab7d`. This test
    /// guards the fork against regressing that fix: it exercises the REAL
    /// `block` matview through `query_and_watch` (the differential PBT's
    /// simplified harness did NOT reproduce the divergence — only the prod
    /// matview does).
    #[tokio::test(flavor = "multi_thread")]
    async fn left_join_isnull_matview_matches_fresh_after_populate_fix() {
        use tokio_stream::StreamExt;
        let engine = create_test_engine().await.unwrap();
        let table = crate::storage::BLOCK_WRITE_TABLE;
        for i in 0..5 {
            engine
                .db_handle()
                .execute(
                    &format!(
                        "INSERT INTO {table} (id, parent_id, content, content_type) VALUES \
                         ('block:lj{i}', 'sentinel:no_parent', 'L{i}', 'text')"
                    ),
                    vec![],
                )
                .await
                .unwrap();
        }
        // lj0 has a requires edge → EXCLUDED by `r.block_id IS NULL` (4 remain).
        engine
            .db_handle()
            .execute(
                "INSERT INTO block_requires (block_id, required_id) VALUES ('block:lj0', 'block:lj1')",
                vec![],
            )
            .await
            .unwrap();
        let sql = "SELECT b.id FROM block b LEFT JOIN block_requires r ON r.block_id = b.id WHERE \
                   r.block_id IS NULL AND b.id LIKE 'block:lj%'";
        let direct = engine
            .db_handle()
            .query(sql, HashMap::new())
            .await
            .unwrap()
            .len();
        let mut stream = engine
            .query_and_watch(sql.to_string(), HashMap::new(), None)
            .await
            .unwrap();
        let first = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .expect("initial batch")
            .expect("a batch");
        let served = first
            .inner
            .items
            .iter()
            .filter(|c| matches!(c.change, holon_api::streaming::Change::Created { .. }))
            .count();
        assert_eq!(
            served, direct,
            "LEFT JOIN…IS NULL matview must match fresh (regressed pre-c6cfab7d: served 5 vs 4)"
        );
        // It stays maintained on the matview path (no degraded disclosure).
        assert_eq!(
            first.metadata.degraded, None,
            "a maintainable shape must NOT be disclosed as degraded"
        );
    }

    #[test]
    fn permanent_vs_transient_matview_error_classifier() {
        // PERMANENT (→ eager backstop). Includes the ACTUAL text measured today
        // on c6cfab7d for a correlated scalar subquery in the SELECT list — the
        // one of the three "not-maintainable" shapes the backstop serves
        // end-to-end (measured: eager + disclosure). The others are R3-era
        // strings kept because they may return once the transform bugs below are
        // fixed and the engine's clean refusal surfaces.
        for permanent in [
            "matview 'watch_view_x' could not be created: Failed to execute DDL: Parse error: \
             Cannot convert LogicalExpr to AST Expr: Case { … }",
            // measured today (SCALAR_SUBQ) — caught, served eager:
            "Correlated scalar subqueries in materialized view SELECT lists are not yet supported \
             by the IVM compiler",
            "INTERSECT and EXCEPT not yet supported in logical plans",
            "Failed to prepare query: Parse error: no such column: x.id",
        ] {
            assert!(
                BackendEngine::is_permanent_matview_conversion_error(&anyhow::anyhow!(
                    "{permanent}"
                )),
                "must be PERMANENT (eager backstop): {permanent}"
            );
        }
        // TRANSIENT (→ propagate, watcher retries): a dependency not built yet.
        // The last two are the ACTUAL errors measured today for EXCEPT and a
        // derived table — they are indistinguishable from a real transient error
        // (SQLite syntax / missing table), so the classifier CANNOT safely route
        // them eager. They therefore WEDGE — but the root cause is a pair of
        // pre-existing holon SQL-transform bugs (EXCEPT → invalid `EXCEPT ALL`;
        // the `_change_origin` transform leaking a derived table's inner alias),
        // triaged OPEN in bugfunnel 2026-08-19-except-transform-emits-except-all
        // and 2026-08-19-change-origin-transform-leaks-derived-alias. Both paths
        // (matview AND eager) hit the mangled SQL, so widening the classifier
        // would not rescue them — the fix is the transforms.
        for transient in [
            "no such table: block_tags_agg",
            "DDL timed out after 30s waiting for dependencies",
            "database is locked",
            "Database schema changed",
            "Failed to execute DDL: near \"ALL\": syntax error",
            "Failed to prepare query: Parse error: no such table: b",
        ] {
            assert!(
                !BackendEngine::is_permanent_matview_conversion_error(&anyhow::anyhow!(
                    "{transient}"
                )),
                "must be TRANSIENT (retry preserved): {transient}"
            );
        }
    }

    /// The wildcard fan-out is not a transition. `*::sync` / `*::full_sync`
    /// carry `entity_name == "*"`, which names no relation, so they lower to no
    /// places; what they do is re-dispatch to each syncable provider.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_wildcard_sync_descriptors_are_not_in_the_net() {
        let engine = create_test_engine().await.expect("test engine");
        let net = engine
            .derived_net()
            .expect("the production catalog compiles");

        for op in ["sync", "full_sync"] {
            let key = holon_net::TransitionKey::operation("*", op);
            assert!(
                net.transition(&key).is_none(),
                "the wildcard `*::{op}` descriptor must not be a transition; the net has {:?}",
                net.transitions
                    .iter()
                    .map(|t| t.key().as_str().to_string())
                    .collect::<Vec<_>>(),
            );
        }
        assert!(
            !net.transitions.is_empty(),
            "the assertion above must not pass by the net being empty",
        );
    }

    /// The engine-synthetic `block` compounds are not dispatcher-registered
    /// providers, so the catalog has to add them explicitly or the net omits
    /// operations that demonstrably fire.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_engine_synthetic_block_compounds_are_in_the_net() {
        let engine = create_test_engine().await.expect("test engine");
        let net = engine
            .derived_net()
            .expect("the production catalog compiles");

        for op in ["convert_block_to_page", "merge_blocks"] {
            assert!(
                engine.has_operation("block", op).await,
                "{op} must be dispatchable for this assertion to mean anything",
            );
            assert!(
                net.transition(&holon_net::TransitionKey::operation("block", op))
                    .is_some(),
                "{op} fires but the net does not describe it",
            );
        }
    }

    /// A rule block the watcher refused enters the net `active: false` +
    /// `Unanalyzable` — declared automation that does not run, never an
    /// absence. Pins the registry → `derive_net` wiring.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_refused_rule_block_is_an_inactive_unanalyzable_transition() {
        let engine = create_test_engine().await.expect("test engine");
        engine.accepted_rules().set(
            "block:rule-broken",
            holon_net::RuleAcceptance::Opaque {
                reason: "parse failed: mapping values are not allowed here".to_string(),
            },
        );

        let net = engine
            .derived_net()
            .expect("the production catalog compiles");
        let transition = net
            .transition(&holon_net::TransitionKey::rule("block:rule-broken"))
            .expect("a refused rule is still declared automation");

        assert!(matches!(
            &transition.source,
            holon_net::TransitionSource::Rule { active: false, .. }
        ));
        assert!(matches!(
            transition.analyzability,
            holon_net::Analyzability::Unanalyzable { .. }
        ));
    }

    /// The sub-fork settled by measurement: two production sites pass different
    /// arguments to `block_synthetic_descriptors` — `di::registration` passes
    /// `false`, `available_operations` passes `template_source.is_some()`. The
    /// net must describe what can FIRE, and `has_operation` is the gate that
    /// decides that, so this asserts the net agrees with the gate rather than
    /// with either literal. Green in both wirings (template source present or
    /// not), which is what makes it a measurement and not a guess.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_net_admits_instantiate_template_exactly_when_dispatch_does() {
        let engine = create_test_engine().await.expect("test engine");
        let dispatchable = engine.has_operation("block", "instantiate_template").await;
        let net = engine
            .derived_net()
            .expect("the production catalog compiles");
        let described = net
            .transition(&holon_net::TransitionKey::operation(
                "block",
                "instantiate_template",
            ))
            .is_some();

        assert_eq!(
            described, dispatchable,
            "the net must describe instantiate_template exactly when the engine will dispatch \
             it; dispatchable={dispatchable}, described={described}",
        );
    }

    /// D31.a recomputes the net on every call and caches nothing, and the
    /// keystone's totality invariant calls it once per tick — so the derive has
    /// to be cheap enough to sit on the tick path. The bound is deliberately
    /// far above the measured cost: it guards against a derive that grows
    /// an I/O or a quadratic scan, not against scheduler noise.
    #[tokio::test(flavor = "multi_thread")]
    async fn deriving_the_net_is_cheap_enough_for_the_tick_path() {
        let engine = create_test_engine().await.expect("test engine");
        engine.derived_net().expect("warm the lazily-built catalog");

        let start = std::time::Instant::now();
        for _ in 0..100 {
            engine
                .derived_net()
                .expect("the production catalog compiles");
        }
        let per_derive = start.elapsed() / 100;
        eprintln!("[net-proj] derived_net() cost: {per_derive:?} per call");
        assert!(
            per_derive < std::time::Duration::from_millis(5),
            "one derive took {per_derive:?}; at that cost a per-tick assertion dominates the \
             keystone and the invariant belongs on the finish hook instead",
        );
    }

    /// A provider advertising ONE descriptor, so a test can put an arbitrary
    /// shape in front of the live catalog.
    struct OneOpProvider(OperationDescriptor);

    #[async_trait::async_trait]
    impl holon_core::OperationProvider for OneOpProvider {
        fn operations(&self) -> Vec<OperationDescriptor> {
            vec![self.0.clone()]
        }

        async fn execute_operation(
            &self,
            _: &EntityName,
            _: &str,
            _: holon_api::StorageEntity,
        ) -> holon_core::Result<holon_core::OperationResult> {
            unreachable!("the net derivation never executes an operation")
        }
    }

    /// Inversion of the fan-out exclusion, against the LIVE engine rather than
    /// the classifier alone: register a provider whose entity wears the
    /// `<provider>.sync` name while actually writing places, and the derive
    /// must refuse it. If the exclusion ever went name-only, this
    /// descriptor would be silently dropped and a real writer would sit
    /// outside every net analysis.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_registered_sync_named_writer_is_refused_by_the_derive() {
        let engine = create_test_engine().await.expect("test engine");
        engine
            .derived_net()
            .expect("the catalog compiles before the writer is registered");

        let mut writer = holon_core::generate_sync_operation("orgmode");
        writer.entity_name = "orgmode.sync".into();
        writer.id_column = "id".to_string();
        writer.affected_fields = vec!["content".to_string()];
        engine
            .get_dispatcher()
            .register_provider(Arc::new(OneOpProvider(writer)))
            .expect("the pair is not yet claimed, so registration succeeds");

        let err = engine
            .derived_net()
            .expect_err("a fan-out-named descriptor that writes places must be refused");
        eprintln!("[net-proj] the derive refused the registered writer: {err}");
        assert!(
            err.to_string().contains("writes places"),
            "the refusal must name the broken premise; got: {err}"
        );
    }
}
