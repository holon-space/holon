//! Holon worker — Phase 1 spike.
//!
//! Goal: falsify or validate H2 (napi build from holon's repo) and H4 (tokio
//! on wasm32-wasip1-threads) before touching any holon code. This crate only
//! exposes two entry points: [`ping`] and [`spawn_check`].
//!
//! See ~/.claude/plans/nifty-bouncing-ladybug.md Phase 1 for the rationale.

#![deny(clippy::future_not_send)]

// Vendored Turso OPFS shim — gated behind `browser` feature.
// See src/turso_browser_shim.rs for provenance and re-sync procedure.
#[cfg(feature = "browser")]
mod turso_browser_shim;

#[cfg(feature = "browser")]
pub use turso_browser_shim::{complete_opfs, init_thread_pool, opfs, Opfs};

#[cfg(feature = "browser")]
mod subscriptions;

#[cfg(feature = "browser")]
mod seed;

use napi_derive::napi;
use parking_lot::Mutex;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// H4 step 1 — tokio timers work inside a `#[napi] async fn`.
///
/// Sleeps 50ms on whatever runtime napi's `tokio_rt` feature installs, then
/// returns a greeting. If the sleep traps, we know napi's ambient runtime
/// is not driving tokio timers on `wasm32-wasip1-threads` and Phase 3 has
/// to fall back to the current-thread/`block_on` path.
#[napi]
pub async fn ping(s: String) -> String {
    tokio::time::sleep(Duration::from_millis(50)).await;
    format!("hello {s}")
}

/// H4 step 2 — current-thread runtime spawns and joins tasks.
///
/// **Original plan:** validate `Builder::new_multi_thread()`. **Result:**
/// FALSIFIED at compile time. tokio 1.51 explicitly refuses to compile
/// `rt-multi-thread` on any wasm target via:
///
/// ```text
/// compile_error!("Only features sync,macros,io-util,rt,time are supported on wasm.");
/// ```
///
/// (See `tokio/src/lib.rs` line 478 in the installed crate version.) That
/// means the worker MUST drive `BackendEngine` from a current-thread runtime.
/// This function builds one and asserts that `tokio::spawn` + `tokio::time`
/// still work on a current-thread driver — which is the entire fallback path
/// the plan called out under H4. If this also fails, the whole architecture
/// has to rethink async.
///
/// Intentionally sync (`#[napi]` without `async`) so the runtime we
/// build here is the one under test, not napi's ambient one.
#[napi]
pub fn spawn_check() -> napi::Result<String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|e| napi::Error::from_reason(format!("runtime build failed: {e}")))?;

    let completed = rt.block_on(async {
        let mut handles = Vec::new();
        for i in 0..4u32 {
            handles.push(tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(10)).await;
                i
            }));
        }
        let mut sum = 0u32;
        for h in handles {
            sum += h.await.expect("join");
        }
        sum
    });

    // 0+1+2+3 = 6 — a wrong answer means a task was dropped silently.
    Ok(format!("current_thread ok: sum={completed} (expected 6)"))
}

// ──────────────────────────────────────────────────────────────────────────
// Phase 3 — BackendEngine bridge
// ──────────────────────────────────────────────────────────────────────────
//
// The worker drives `BackendEngine` from a single dedicated current-thread
// tokio runtime. Every `#[napi]` async fn enters that runtime via
// `block_on` so napi's ambient runtime never sees holon's futures (which
// are not guaranteed `Send` in all paths and would tie us to a runtime
// scheduler we can't customize).
//
// On `wasm32-wasip1-threads` `block_on` parks via `Atomics.wait`, which is
// allowed because the entire wasm instance lives inside a dedicated Web
// Worker (Phase 1 H4 finding).

#[cfg(feature = "browser")]
mod backend {
    use super::*;
    use holon::api::backend_engine::{BackendEngine, QueryContext};
    use holon::api::holon_service::HolonService;
    use holon::di::lifecycle::create_backend_engine;
    use holon::storage::types::StorageEntity;
    use holon_api::{Change, EntityUri, QueryLanguage, Value};
    use holon_frontend::command_provider::CommandProvider;
    use holon_frontend::reactive::{BuilderServices, ReactiveEngine};
    use holon_frontend::shadow_builders::build_shadow_interpreter;
    use holon_frontend::{interpret_pure, FrontendSession, ReactiveViewModel};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    pub(super) struct EngineState {
        pub engine: Arc<BackendEngine>,
        /// Current-thread runtime. Arc so callers can clone before dropping
        /// the mutex guard (lock discipline: never hold guard across block_on).
        pub runtime: Arc<tokio::runtime::Runtime>,
        pub session: Arc<FrontendSession>,
        pub reactive: Arc<ReactiveEngine>,
    }

    static ENGINE: OnceLock<Mutex<Option<EngineState>>> = OnceLock::new();

    // ── MCP watch registry ────────────────────────────────────────────────────
    // Stores per-watch pending changes buffers and background drain tasks.
    // Separate from `subscriptions` (which only tracks ReactiveEngine watchers).

    struct McpWatchEntry {
        pending: Arc<parking_lot::Mutex<Vec<serde_json::Value>>>,
        _task: tokio::task::JoinHandle<()>,
    }

    static MCP_WATCHES: OnceLock<parking_lot::Mutex<HashMap<String, McpWatchEntry>>> =
        OnceLock::new();
    static MCP_WATCH_COUNTER: AtomicU32 = AtomicU32::new(1);

    fn mcp_watches() -> &'static parking_lot::Mutex<HashMap<String, McpWatchEntry>> {
        MCP_WATCHES.get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
    }

    pub(super) fn slot() -> &'static Mutex<Option<EngineState>> {
        ENGINE.get_or_init(|| Mutex::new(None))
    }

    /// Initialize the global `BackendEngine` + `FrontendSession` + `ReactiveEngine`
    /// against the OPFS-backed DB at `db_path`. Idempotent: a second call replaces
    /// the state.
    pub(super) fn init(db_path: String) -> napi::Result<()> {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .map_err(|e| super::nerr("runtime build", e))?,
        );

        let path = PathBuf::from(db_path);
        let engine = runtime
            .block_on(async { create_backend_engine(path, |_| Ok(())).await })
            .map_err(|e| super::nerr("create_backend_engine", e))?;

        // Seed default layout blocks if none exist yet (idempotent).
        runtime
            .block_on(async { super::seed::seed_default_layout(&engine).await })
            .map_err(|e| super::nerr("seed_default_layout", e))?;

        let session = Arc::new(FrontendSession::from_engine(engine.clone()));

        // OnceLock breaks the circular dep: ReactiveEngine needs itself as
        // BuilderServices inside interpret_fn, but exists only after construction.
        let services_slot: Arc<std::sync::OnceLock<Arc<dyn BuilderServices>>> =
            Arc::new(std::sync::OnceLock::new());
        let slot_clone = services_slot.clone();
        let reactive = Arc::new(ReactiveEngine::new(
            session.clone(),
            runtime.handle().clone(),
            Arc::new(build_shadow_interpreter()),
            move |expr, rows| match slot_clone.get() {
                Some(s) => interpret_pure(expr, rows, &**s),
                None => ReactiveViewModel::empty(),
            },
        ));
        let services: Arc<dyn BuilderServices> = reactive.clone();
        // Ignore the error — if already set, a previous init wired things up.
        let _ = services_slot.set(services);

        *slot().lock() = Some(EngineState {
            engine,
            runtime,
            session,
            reactive,
        });
        Ok(())
    }

    /// Extract `(Arc<BackendEngine>, Arc<Runtime>)` without holding the lock
    /// across `block_on`. Panics if the engine is not initialized.
    fn engine_and_rt(
        label: &str,
    ) -> napi::Result<(Arc<BackendEngine>, Arc<tokio::runtime::Runtime>)> {
        let guard = slot().lock();
        let state = guard
            .as_ref()
            .ok_or_else(|| super::nerr(label, "engine not initialized"))?;
        Ok((state.engine.clone(), state.runtime.clone()))
    }

    /// Extract `(Arc<ReactiveEngine>, Arc<Runtime>)` without holding the lock.
    fn reactive_and_rt(
        label: &str,
    ) -> napi::Result<(Arc<ReactiveEngine>, Arc<tokio::runtime::Runtime>)> {
        let guard = slot().lock();
        let state = guard
            .as_ref()
            .ok_or_else(|| super::nerr(label, "engine not initialized"))?;
        Ok((state.reactive.clone(), state.runtime.clone()))
    }

    /// Execute a SQL statement (NOT PRQL) via `BackendEngine::execute_query`.
    /// Returns rows as a serde_json::Value array, then serialized to a
    /// JSON string for the JS bridge.
    pub(super) fn execute_query(sql: String) -> napi::Result<String> {
        let (engine, runtime) = engine_and_rt("execute_query")?;
        let rows: Vec<HashMap<String, Value>> = runtime
            .block_on(async {
                engine
                    .execute_query(sql, HashMap::new(), Some(QueryContext::root()))
                    .await
            })
            .map_err(|e| super::nerr("execute_query", e))?;
        serde_json::to_string(&rows).map_err(|e| super::nerr("serialize rows", e))
    }

    /// Execute a DDL/DML statement (CREATE TABLE, INSERT, UPDATE, DELETE).
    ///
    /// Does NOT return affected row count — `DbHandle::query` is the only
    /// entry point that accepts named params, and it doesn't expose
    /// `changes()`. If callers need the count, add a new accessor on
    /// `DbHandle` rather than lying about the return type.
    pub(super) fn execute_sql(sql: String) -> napi::Result<()> {
        let (engine, runtime) = engine_and_rt("execute_sql")?;
        runtime
            .block_on(async {
                engine.db_handle().query(&sql, HashMap::new()).await?;
                Ok::<_, anyhow::Error>(())
            })
            .map_err(|e| super::nerr("execute_sql", e))
    }

    /// Drive the current-thread runtime for a short time slice so spawned
    /// tasks (notably `watch_view` drain loops) can make progress.
    ///
    /// Required because `Builder::new_current_thread()` has no background
    /// driver — nothing runs unless someone calls `block_on`. JS callers
    /// should invoke this from a `setTimeout` / `setInterval` loop (or
    /// after every user action) so watchers actually deliver snapshots.
    ///
    /// `budget_ms` bounds the sleep inside `block_on`; the reactor will
    /// still drain any tasks that become ready and then return.
    pub(super) fn tick(budget_ms: u32) -> napi::Result<()> {
        let (_, runtime) = engine_and_rt("tick")?;
        let budget = std::time::Duration::from_millis(u64::from(budget_ms));
        runtime.block_on(async move {
            tokio::time::sleep(budget).await;
        });
        Ok(())
    }

    /// B2 validation: call `watch_live` on the root layout block and return
    /// a summary string. Logs the kind of the root node to confirm the
    /// reactive pipeline is wired end-to-end.
    pub(super) fn reactive_check() -> napi::Result<String> {
        let (reactive, _runtime) = reactive_and_rt("reactive_check")?;
        let root_uri = holon_api::root_layout_block_uri();
        let services: Arc<dyn BuilderServices> = reactive.clone();
        let live_block = reactive.watch_live(&root_uri, services);

        let snapshot = live_block.tree.snapshot();
        let kind_name = snapshot.kind.tag();
        tracing::info!("[engine_reactive_check] root kind={kind_name}");
        Ok(format!("reactive check ok: root_kind={kind_name}"))
    }

    /// B3: start watching `block_id`, fire `callback(snapshotJson)` on every
    /// `ViewModel` change.  Returns a handle ID for `drop_subscription`.
    ///
    /// The snapshot is a full `serde_json`-serialized `ViewModel`.  No diff.
    /// Bursts are coalesced — if multiple snapshots are ready between
    /// drains, only the latest is serialized and delivered.
    ///
    /// The callback is wrapped in a `ThreadsafeFunction` so it can be called
    /// from the tokio task that drains the `ReactiveEngine::watch` stream.
    ///
    /// Fail-loud: a serialize error tears down the subscription. Silent
    /// log-and-continue is a CLAUDE.md violation and would hide real bugs
    /// in `ViewModel`.
    pub(super) fn watch_view(
        block_id: String,
        callback: napi::bindgen_prelude::Function<'_, (String,), ()>,
    ) -> napi::Result<u32> {
        use futures::StreamExt;
        use napi::threadsafe_function::{ThreadsafeCallContext, ThreadsafeFunctionCallMode};

        let tsfn = Arc::new(
            callback
                .build_threadsafe_function::<String>()
                .build_callback(|ctx: ThreadsafeCallContext<String>| Ok((ctx.value,)))?,
        );

        let (reactive, runtime) = reactive_and_rt("watch_view")?;
        let block_uri = holon_api::EntityUri::from_raw(&block_id);

        // Allocate handle BEFORE spawning so the task can self-remove on
        // stream end and the return value is deterministic (no TSFN-async
        // ordering dance in JS).
        let handle = crate::subscriptions::allocate();

        let task = runtime.spawn(async move {
            // Fail-loud helper: serialize + call. On serialize failure,
            // panic — the task aborts and the bug surfaces instead of
            // silently swallowing a malformed ViewModel.
            let emit = |rvm: &holon_frontend::ReactiveViewModel| {
                let snapshot = rvm.snapshot();
                let json = serde_json::to_string(&snapshot).unwrap_or_else(|e| {
                    panic!("[watch_view handle={handle}] ViewModel serialize failed: {e}")
                });
                tsfn.call(json, ThreadsafeFunctionCallMode::NonBlocking);
            };

            let mut stream = reactive.watch(&block_uri);
            while let Some(mut rvm) = stream.next().await {
                // Coalesce: drain any immediately-available snapshots and
                // keep only the latest. `futures::poll!` peeks without
                // awaiting. A burst of structural + data events collapses
                // to a single serialize + TSFN call.
                loop {
                    match futures::poll!(stream.next()) {
                        std::task::Poll::Ready(Some(next)) => rvm = next,
                        std::task::Poll::Ready(None) => {
                            emit(&rvm);
                            crate::subscriptions::remove(handle);
                            tracing::debug!(
                                "[watch_view] stream ended (coalesced) for {block_uri}"
                            );
                            return;
                        }
                        std::task::Poll::Pending => break,
                    }
                }
                emit(&rvm);
            }
            crate::subscriptions::remove(handle);
            tracing::debug!("[watch_view] stream ended for {block_uri}");
        });

        crate::subscriptions::install(handle, task);
        Ok(handle)
    }

    /// B3: cancel a subscription previously started with `watch_view`.
    pub(super) fn drop_subscription(handle: u32) {
        crate::subscriptions::cancel(handle);
    }

    /// B4: dispatch an operation via FrontendSession and return the result as JSON.
    ///
    /// `params_json` is a JSON object string (e.g. `{"id":"block:foo"}`).
    /// Returns `"null"` when the operation returns `None`.
    pub(super) fn execute_operation(
        entity: String,
        op: String,
        params_json: String,
    ) -> napi::Result<String> {
        let params: HashMap<String, holon_api::Value> =
            serde_json::from_str(&params_json).map_err(|e| super::nerr("parse params", e))?;

        let guard = slot().lock();
        let state = guard
            .as_ref()
            .ok_or_else(|| super::nerr("execute_operation", "engine not initialized"))?;
        let session = state.session.clone();
        let runtime = state.runtime.clone();
        drop(guard);

        let result = runtime
            .block_on(async { session.execute_operation(&entity, &op, params).await })
            .map_err(|e| super::nerr("execute_operation", e))?;

        serde_json::to_string(&result).map_err(|e| super::nerr("serialize result", e))
    }

    /// B4: switch the active render variant for a watched block.
    pub(super) fn set_variant(block_id: String, variant: String) -> napi::Result<()> {
        let (reactive, runtime) = reactive_and_rt("set_variant")?;
        let block_uri = holon_api::EntityUri::from_raw(&block_id);
        runtime
            .block_on(async { reactive.set_variant(&block_uri, variant).await })
            .map_err(|e| super::nerr("set_variant", e))
    }

    // ── MCP tool bridge ───────────────────────────────────────────────────────

    /// Await the reactive engine becoming ready for `block_id`, then return its
    /// current `ViewModel` snapshot as a JSON string. Up to 5 s timeout.
    pub(super) fn snapshot_view(block_id: String) -> napi::Result<String> {
        let (reactive, runtime) = reactive_and_rt("snapshot_view")?;
        let block_uri = EntityUri::from_raw(&block_id);
        runtime.block_on(async {
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                reactive.await_ready(&block_uri),
            )
            .await;
        });
        let snapshot = reactive.snapshot(&block_uri);
        serde_json::to_string(&snapshot).map_err(|e| super::nerr("serialize snapshot", e))
    }

    /// Dispatch an MCP tool call by name, accepting args as a JSON string and
    /// returning the result as a JSON string. Used by the browser relay bridge.
    pub(super) fn mcp_tool(name: String, args_json: String) -> napi::Result<String> {
        let args: serde_json::Value =
            serde_json::from_str(&args_json).map_err(|e| super::nerr("parse args", e))?;

        let guard = slot().lock();
        let state = guard
            .as_ref()
            .ok_or_else(|| super::nerr(&name, "engine not initialized"))?;
        let engine = state.engine.clone();
        let runtime = state.runtime.clone();
        let reactive = state.reactive.clone();
        drop(guard);

        let service = HolonService::new(engine.clone());

        let result = dispatch_mcp_tool(&name, args, &engine, &service, &reactive, &runtime)
            .map_err(|e| napi::Error::from_reason(format!("mcp_tool::{name}: {e}")))?;

        serde_json::to_string(&result).map_err(|e| super::nerr("serialize result", e))
    }

    #[allow(clippy::too_many_arguments)]
    fn dispatch_mcp_tool(
        name: &str,
        args: serde_json::Value,
        engine: &Arc<BackendEngine>,
        service: &HolonService,
        reactive: &Arc<ReactiveEngine>,
        runtime: &Arc<tokio::runtime::Runtime>,
    ) -> anyhow::Result<serde_json::Value> {
        match name {
            "execute_query" => {
                let query = req_str(&args, "query")?;
                let language = args
                    .get("language")
                    .and_then(|v| v.as_str())
                    .unwrap_or("holon_sql")
                    .to_string();
                let lang = language.parse::<QueryLanguage>()?;
                let params = parse_params(&args);
                let context_id = args
                    .get("context_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                let context_parent_id = args
                    .get("context_parent_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);

                let result = runtime.block_on(async {
                    let context = service
                        .build_context(context_id.as_deref(), context_parent_id.as_deref())
                        .await;
                    service.execute_query(&query, lang, params, context).await
                })?;

                let rows: Vec<serde_json::Value> = result
                    .rows
                    .iter()
                    .map(|row| serde_json::to_value(row).unwrap_or(serde_json::Value::Null))
                    .collect();
                let count = rows.len();
                Ok(serde_json::json!({
                    "rows": rows,
                    "row_count": count,
                    "duration_ms": result.duration.as_secs_f64() * 1000.0,
                }))
            }

            "execute_raw_sql" => {
                let sql = req_str(&args, "sql")?;
                let params = parse_params(&args);
                let result = runtime.block_on(service.execute_raw_sql(&sql, params))?;
                let rows: Vec<serde_json::Value> = result
                    .rows
                    .iter()
                    .map(|row| serde_json::to_value(row).unwrap_or(serde_json::Value::Null))
                    .collect();
                let count = rows.len();
                Ok(serde_json::json!({"rows": rows, "row_count": count}))
            }

            "compile_query" => {
                let query = req_str(&args, "query")?;
                let language = req_str(&args, "language")?;
                let lang = language.parse::<QueryLanguage>()?;
                let compiled = service.compile_query(&query, lang)?;
                Ok(serde_json::json!({"compiled_sql": compiled, "render_spec": null}))
            }

            "list_tables" => {
                let listing = runtime.block_on(service.list_tables())?;
                let tables: Vec<serde_json::Value> = listing
                    .tables
                    .iter()
                    .map(|t| serde_json::json!({"name": t.name, "type": "table", "definition": t.definition}))
                    .collect();
                let views: Vec<serde_json::Value> = listing
                    .views
                    .iter()
                    .map(|v| serde_json::json!({"name": v.name, "type": "view", "definition": v.definition}))
                    .collect();
                let matviews: Vec<serde_json::Value> = listing
                    .materialized_views
                    .iter()
                    .map(|m| serde_json::json!({"name": m.name, "type": "materialized_view", "definition": m.definition}))
                    .collect();
                Ok(serde_json::json!({
                    "tables": tables,
                    "views": views,
                    "materialized_views": matviews,
                }))
            }

            "create_table" => {
                let table_name = req_str(&args, "table_name")?;
                let cols_val = args
                    .get("columns")
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| anyhow::anyhow!("missing required field 'columns'"))?;
                let cols: Vec<holon::api::holon_service::ColumnDef> = cols_val
                    .iter()
                    .map(|c| holon::api::holon_service::ColumnDef {
                        name: c
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        sql_type: c
                            .get("sql_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("TEXT")
                            .to_string(),
                        primary_key: c
                            .get("primary_key")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                        default: c
                            .get("default")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                    })
                    .collect();
                runtime.block_on(service.create_table(&table_name, &cols))?;
                Ok(serde_json::json!({"success": true}))
            }

            "insert_data" => {
                let table_name = req_str(&args, "table_name")?;
                let rows_val = args
                    .get("rows")
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| anyhow::anyhow!("missing required field 'rows'"))?;
                let rows: Vec<HashMap<String, Value>> = rows_val
                    .iter()
                    .map(|row| {
                        row.as_object()
                            .map(|obj| {
                                obj.iter()
                                    .map(|(k, v)| (k.clone(), Value::from_json_value(v.clone())))
                                    .collect()
                            })
                            .unwrap_or_default()
                    })
                    .collect();
                let count = runtime.block_on(service.insert_data(&table_name, &rows))?;
                Ok(serde_json::json!({"inserted": count}))
            }

            "drop_table" => {
                let table_name = req_str(&args, "table_name")?;
                runtime.block_on(service.drop_table(&table_name))?;
                Ok(serde_json::json!({"success": true}))
            }

            "watch_query" => {
                use futures::StreamExt as _;

                let query = req_str(&args, "query")?;
                let language = args
                    .get("language")
                    .and_then(|v| v.as_str())
                    .unwrap_or("holon_prql")
                    .to_string();
                let lang = language.parse::<QueryLanguage>()?;
                let params = parse_params(&args);

                let mut stream =
                    runtime.block_on(service.query_and_watch(&query, lang, params, None))?;

                // First batch delivers initial data (Created items).
                let initial_rows: Vec<HashMap<String, Value>> = runtime.block_on(async {
                    if let Some(first_batch) = stream.next().await {
                        first_batch
                            .inner
                            .items
                            .into_iter()
                            .filter_map(|rc| {
                                if let Change::Created { data, .. } = rc.change {
                                    Some(data)
                                } else {
                                    None
                                }
                            })
                            .collect()
                    } else {
                        vec![]
                    }
                });

                let json_initial: Vec<serde_json::Value> = initial_rows
                    .iter()
                    .map(|row| serde_json::to_value(row).unwrap_or(serde_json::Value::Null))
                    .collect();

                let watch_id = format!(
                    "mcp-watch-{}",
                    MCP_WATCH_COUNTER.fetch_add(1, Ordering::Relaxed)
                );
                let pending = Arc::new(parking_lot::Mutex::new(Vec::<serde_json::Value>::new()));
                let pending_clone = pending.clone();

                // Drain task: accumulates CDC batches into the pending buffer.
                // Only runs when block_on is called (current-thread runtime).
                let task = runtime.spawn(async move {
                    use futures::StreamExt as _;
                    while let Some(batch) = stream.next().await {
                        let mut buf = pending_clone.lock();
                        for row_change in batch.inner.items {
                            buf.push(change_to_json(row_change.change));
                        }
                    }
                });

                mcp_watches().lock().insert(
                    watch_id.clone(),
                    McpWatchEntry {
                        pending,
                        _task: task,
                    },
                );

                Ok(serde_json::json!({"watch_id": watch_id, "initial_data": json_initial}))
            }

            "poll_changes" => {
                let watch_id = req_str(&args, "watch_id")?;
                // Yield to the reactor so drain tasks can process any ready CDC events.
                runtime.block_on(tokio::time::sleep(std::time::Duration::ZERO));
                let guard = mcp_watches().lock();
                let entry = guard
                    .get(&watch_id)
                    .ok_or_else(|| anyhow::anyhow!("watch '{}' not found", watch_id))?;
                let changes: Vec<serde_json::Value> = entry.pending.lock().drain(..).collect();
                Ok(serde_json::to_value(changes)?)
            }

            "stop_watch" => {
                let watch_id = req_str(&args, "watch_id")?;
                mcp_watches().lock().remove(&watch_id);
                Ok(serde_json::json!({"success": true}))
            }

            "describe_ui" => {
                let block_id = req_str(&args, "block_id")?;
                let format = args
                    .get("format")
                    .and_then(|v| v.as_str())
                    .unwrap_or("text")
                    .to_string();
                let block_uri = EntityUri::from_raw(&block_id);
                runtime.block_on(async {
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        reactive.await_ready(&block_uri),
                    )
                    .await;
                });
                let snapshot = reactive.snapshot(&block_uri);
                let text = if format == "json" {
                    serde_json::to_string_pretty(&snapshot)?
                } else {
                    snapshot.pretty_print(0)
                };
                Ok(serde_json::Value::String(text))
            }

            "rank_tasks" => {
                let result = runtime.block_on(service.rank_tasks())?;
                let tasks: Vec<serde_json::Value> = result
                    .ranked
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        serde_json::json!({
                            "rank": i + 1,
                            "block_id": t.block_id,
                            "label": t.label,
                            "delta_obj": t.delta_obj,
                            "delta_per_minute": t.delta_per_minute,
                            "duration_minutes": t.duration_minutes,
                        })
                    })
                    .collect();
                Ok(serde_json::json!({
                    "tasks": tasks,
                    "mental_slots": {
                        "occupied": result.mental_slots.occupied,
                        "capacity": result.mental_slots.capacity,
                    },
                }))
            }

            "undo" => {
                let success = runtime.block_on(service.undo())?;
                Ok(serde_json::json!({
                    "success": success,
                    "message": if success { "Undo successful" } else { "Nothing to undo" },
                }))
            }
            "redo" => {
                let success = runtime.block_on(service.redo())?;
                Ok(serde_json::json!({
                    "success": success,
                    "message": if success { "Redo successful" } else { "Nothing to redo" },
                }))
            }
            "can_undo" => {
                let available = runtime.block_on(async { service.can_undo().await });
                Ok(serde_json::json!({"available": available}))
            }
            "can_redo" => {
                let available = runtime.block_on(async { service.can_redo().await });
                Ok(serde_json::json!({"available": available}))
            }

            "list_operations" => {
                let entity_name = req_str(&args, "entity_name")?;
                let ops = runtime.block_on(service.available_operations(&entity_name));
                Ok(serde_json::to_value(&ops)?)
            }

            "execute_operation" => {
                let entity_name = req_str(&args, "entity_name")?;
                let operation = req_str(&args, "operation")?;
                let storage_entity: StorageEntity = args
                    .get("params")
                    .and_then(|v| v.as_object())
                    .map(|obj| {
                        obj.iter()
                            .map(|(k, v)| (k.clone(), Value::from_json_value(v.clone())))
                            .collect()
                    })
                    .unwrap_or_default();
                let result = runtime.block_on(service.execute_operation(
                    &entity_name,
                    &operation,
                    storage_entity,
                ))?;
                Ok(serde_json::to_value(&result)?)
            }

            "list_commands" => {
                let block_id = req_str(&args, "block_id")?;
                let filter = args
                    .get("filter")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let block_uri = EntityUri::parse(&block_id)?;

                let block_result = runtime.block_on(service.execute_raw_sql(
                    "SELECT * FROM block WHERE id = $1",
                    HashMap::from([("1".to_string(), Value::String(block_uri.to_string()))]),
                ))?;

                let context_params: HashMap<String, Value> =
                    block_result.rows.first().cloned().unwrap_or_default();

                let profile = block_result
                    .rows
                    .first()
                    .map(|row| engine.profile_resolver().resolve(row));
                let entity_name = profile
                    .as_ref()
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "blocks".to_string());

                let ops = runtime.block_on(service.available_operations(&entity_name));
                let wirings: Vec<holon_api::render_types::OperationWiring> = ops
                    .into_iter()
                    .map(|d| holon_api::render_types::OperationWiring {
                        modified_param: String::new(),
                        descriptor: d,
                    })
                    .collect();

                let commands =
                    CommandProvider::build_command_items(&wirings, &context_params, &filter);
                let result: Vec<serde_json::Value> = commands
                    .iter()
                    .map(|item| {
                        serde_json::json!({
                            "name": item.id,
                            "display_name": item.label,
                            "entity_name": entity_name,
                        })
                    })
                    .collect();
                Ok(serde_json::to_value(result)?)
            }

            "execute_command" => {
                let block_id = req_str(&args, "block_id")?;
                let command_name = req_str(&args, "command_name")?;
                let entity_name = req_str(&args, "entity_name")?;
                let mut storage_entity: StorageEntity = args
                    .get("params")
                    .and_then(|v| v.as_object())
                    .map(|obj| {
                        obj.iter()
                            .map(|(k, v)| (k.clone(), Value::from_json_value(v.clone())))
                            .collect()
                    })
                    .unwrap_or_default();
                storage_entity
                    .entry("id".to_string())
                    .or_insert_with(|| Value::String(block_id.clone()));
                let result = runtime.block_on(service.execute_operation(
                    &entity_name,
                    &command_name,
                    storage_entity,
                ))?;
                Ok(serde_json::to_value(&result)?)
            }

            // Loro/org tools and GPUI-specific tools are not available in the browser worker.
            "inspect_loro_blocks"
            | "diff_loro_sql"
            | "list_loro_documents"
            | "read_org_file"
            | "render_org_from_blocks"
            | "create_entity_type"
            | "screenshot"
            | "click"
            | "scroll"
            | "type_text"
            | "send_key_chord"
            | "send_navigation"
            | "describe_navigation" => Err(anyhow::anyhow!(
                "tool '{}' is not available in browser worker mode",
                name
            )),

            _ => Err(anyhow::anyhow!("unknown tool '{}'", name)),
        }
    }

    fn req_str(args: &serde_json::Value, field: &str) -> anyhow::Result<String> {
        args.get(field)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("missing required field '{}'", field))
    }

    fn parse_params(args: &serde_json::Value) -> HashMap<String, Value> {
        args.get("params")
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .map(|(k, v)| (k.clone(), Value::from_json_value(v.clone())))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn change_to_json(change: Change<StorageEntity>) -> serde_json::Value {
        match change {
            Change::Created { data, .. } => {
                let entity_id = data.get("id").and_then(|v| v.as_string_owned());
                serde_json::json!({
                    "change_type": "Created",
                    "entity_id": entity_id,
                    "data": serde_json::to_value(&data).unwrap_or(serde_json::Value::Null),
                })
            }
            Change::Updated { id, data, .. } => serde_json::json!({
                "change_type": "Updated",
                "entity_id": id,
                "data": serde_json::to_value(&data).unwrap_or(serde_json::Value::Null),
            }),
            Change::Deleted { id, .. } => serde_json::json!({
                "change_type": "Deleted",
                "entity_id": id,
                "data": null,
            }),
            Change::FieldsChanged {
                entity_id, fields, ..
            } => {
                let data: serde_json::Map<String, serde_json::Value> = fields
                    .into_iter()
                    .map(|(field_name, _old, new_val)| {
                        (
                            field_name,
                            serde_json::to_value(&new_val).unwrap_or(serde_json::Value::Null),
                        )
                    })
                    .collect();
                serde_json::json!({
                    "change_type": "Updated",
                    "entity_id": entity_id,
                    "data": data,
                })
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Phase 3 — napi exports for BackendEngine + ReactiveEngine
// ──────────────────────────────────────────────────────────────────────────
//
// All engine-facing exports live under one `cfg(feature = "browser")`
// gate to avoid repeating the attribute on every function.

#[cfg(feature = "browser")]
mod engine_exports {
    use super::backend;

    #[napi_derive::napi]
    pub fn engine_init(db_path: String) -> napi::Result<()> {
        backend::init(db_path)
    }

    #[napi_derive::napi]
    pub fn engine_execute_query(sql: String) -> napi::Result<String> {
        backend::execute_query(sql)
    }

    /// Execute a DDL/DML statement. Returns `()` — affected row count is
    /// not exposed yet (see `backend::execute_sql`).
    #[napi_derive::napi]
    pub fn engine_execute_sql(sql: String) -> napi::Result<()> {
        backend::execute_sql(sql)
    }

    /// B2 validation: run `watch_live` on the root layout block and return
    /// the root node variant tag.
    #[napi_derive::napi]
    pub fn engine_reactive_check() -> napi::Result<String> {
        backend::reactive_check()
    }

    /// Drive the current-thread runtime for `budget_ms` milliseconds so
    /// spawned tasks (notably `watch_view` drains) can progress. Must be
    /// called periodically from JS (`setInterval` / `requestAnimationFrame`
    /// / after every RPC) — nothing runs without a `block_on` caller.
    #[napi_derive::napi]
    pub fn engine_tick(budget_ms: u32) -> napi::Result<()> {
        backend::tick(budget_ms)
    }

    /// Start watching `block_id` for `ViewModel` changes.
    ///
    /// `callback` receives a single string argument — the full JSON-serialized
    /// `ViewModel` snapshot — on every structural or data change. Bursts
    /// are coalesced to the latest value.
    ///
    /// Returns a `u32` handle to pass to `engine_drop_subscription`. The
    /// handle is registered BEFORE the drain task is spawned, so JS can
    /// rely on the returned value without any ordering dance.
    #[napi_derive::napi]
    pub fn engine_watch_view(
        block_id: String,
        callback: napi::bindgen_prelude::Function<'_, (String,), ()>,
    ) -> napi::Result<u32> {
        backend::watch_view(block_id, callback)
    }

    /// Cancel a subscription created by `engine_watch_view`. No-op if the
    /// stream already ended naturally.
    #[napi_derive::napi]
    pub fn engine_drop_subscription(handle: u32) {
        backend::drop_subscription(handle)
    }

    /// Execute a holon operation and return the result as a JSON string.
    ///
    /// - `entity`: entity name (e.g. `"block"`)
    /// - `op`: operation name (e.g. `"update"`, `"delete"`, `"create"`)
    /// - `params_json`: JSON-encoded `{ key: value }` params map
    ///
    /// Returns the JSON-serialized `Option<Value>` result: `"null"` when
    /// the operation returns nothing, or a JSON-encoded `Value` otherwise.
    #[napi_derive::napi]
    pub fn engine_execute_operation(
        entity: String,
        op: String,
        params_json: String,
    ) -> napi::Result<String> {
        backend::execute_operation(entity, op, params_json)
    }

    /// Switch the active render variant for a watched block. Errors if the
    /// block is not currently being watched (see
    /// `ReactiveEngine::set_variant`).
    #[napi_derive::napi]
    pub fn engine_set_variant(block_id: String, variant: String) -> napi::Result<()> {
        backend::set_variant(block_id, variant)
    }

    /// Await the block's reactive ViewModel becoming ready (up to 5 s), then
    /// return the current snapshot as a JSON string.
    ///
    /// Used by the browser MCP relay bridge for the `describe_ui` tool when
    /// called before the block has been watched by any UI component.
    #[napi_derive::napi]
    pub fn engine_snapshot_view(block_id: String) -> napi::Result<String> {
        backend::snapshot_view(block_id)
    }

    /// Dispatch an MCP tool call by name. `args_json` is a JSON object string
    /// with the tool's arguments. Returns the result as a JSON string.
    ///
    /// Used by the Dioxus page relay bridge: it receives `{id, tool, arguments}`
    /// over WebSocket from the native relay, serialises `arguments` to a JSON
    /// string, and calls this function.
    #[napi_derive::napi]
    pub fn engine_mcp_tool(name: String, args_json: String) -> napi::Result<String> {
        backend::mcp_tool(name, args_json)
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Phase 2 — minimal Turso wrapper over OPFS (kept for the smoke harness)
// ──────────────────────────────────────────────────────────────────────────

#[cfg(feature = "browser")]
struct DbState {
    _db: Arc<turso_core::Database>,
    conn: Arc<turso_core::Connection>,
    io: Arc<dyn turso_core::IO>,
}

#[cfg(feature = "browser")]
static DB: OnceLock<Mutex<Option<DbState>>> = OnceLock::new();

#[cfg(feature = "browser")]
fn db_slot() -> &'static Mutex<Option<DbState>> {
    DB.get_or_init(|| Mutex::new(None))
}

fn nerr(label: &str, e: impl std::fmt::Display) -> napi::Error {
    napi::Error::from_reason(format!("{label}: {e}"))
}

/// Open (or create) a Turso database at `path` backed by the OPFS IO shim.
/// Re-opening replaces any existing handle. Phase 2 does not yet support
/// multiple databases.
#[cfg(feature = "browser")]
#[napi]
pub fn open_db(path: String) -> napi::Result<()> {
    // NOTE: the Turso IO loop inside open_file_with_flags drives IO
    // completions via `Completion::wait`, which on the OPFS shim returns to
    // JS via `ioNotifier.waitForCompletion()` — already async-safe inside a
    // Web Worker because the worker can block synchronously.
    let io: Arc<dyn turso_core::IO> = turso_browser_shim::opfs();
    let core_opts = turso_core::DatabaseOpts::new();
    let flags = turso_core::OpenFlags::Create;

    let db = turso_core::Database::open_file_with_flags(io.clone(), &path, flags, core_opts, None)
        .map_err(|e| nerr("open_file_with_flags", e))?;

    let conn = db.connect().map_err(|e| nerr("connect", e))?;

    *db_slot().lock() = Some(DbState { _db: db, conn, io });
    Ok(())
}

/// Execute a DDL/DML statement, returning affected row count.
#[cfg(feature = "browser")]
#[napi]
pub fn db_execute(sql: String) -> napi::Result<i64> {
    let guard = db_slot().lock();
    let state = guard
        .as_ref()
        .ok_or_else(|| nerr("db_execute", "db not open"))?;
    let mut stmt = state.conn.prepare(&sql).map_err(|e| nerr("prepare", e))?;
    stmt.run_ignore_rows()
        .map_err(|e| nerr("run_ignore_rows", e))?;
    Ok(state.conn.changes())
}

/// Execute a SELECT and return rows as newline-delimited `|`-joined strings.
/// Phase 2 only needs a string round-trip to prove persistence; a proper
/// typed JSON path lands in Phase 3 alongside the BackendEngine wiring.
#[cfg(feature = "browser")]
#[napi]
pub fn db_query(sql: String) -> napi::Result<String> {
    let guard = db_slot().lock();
    let state = guard
        .as_ref()
        .ok_or_else(|| nerr("db_query", "db not open"))?;
    let mut stmt = state.conn.prepare(&sql).map_err(|e| nerr("prepare", e))?;

    let rows = stmt
        .run_collect_rows()
        .map_err(|e| nerr("run_collect_rows", e))?;

    let mut out = String::new();
    for row in rows {
        for (i, v) in row.iter().enumerate() {
            if i > 0 {
                out.push('|');
            }
            out.push_str(&format!("{v:?}"));
        }
        out.push('\n');
    }
    Ok(out)
}

// ──────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────
//
// Native-only tests. The wasm build has no test harness; these exist so
// `cargo test -p holon-worker` catches regressions in the serde glue
// before hitting the worker build.

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use holon_api::Value;
    use std::collections::HashMap;

    /// `execute_operation` deserializes `params_json` into
    /// `HashMap<String, holon_api::Value>`. `Value` is `#[serde(untagged)]`,
    /// so plain JSON primitives (strings, numbers, bools) must round-trip
    /// without any discriminator tags on the JS side. If this ever breaks
    /// — e.g. someone adds `#[serde(tag = "type")]` to Value — every
    /// frontend call to `engine_execute_operation` starts failing at
    /// runtime. Lock it in here.
    #[test]
    fn execute_operation_params_round_trip_untagged_json() {
        let js_input = r#"{
            "id": "block:foo",
            "content": "hello world",
            "priority": 3,
            "done": true,
            "ratio": 0.5
        }"#;
        let parsed: HashMap<String, Value> =
            serde_json::from_str(js_input).expect("parse params_json");

        assert_eq!(parsed["id"], Value::String("block:foo".into()));
        assert_eq!(parsed["content"], Value::String("hello world".into()));
        assert_eq!(parsed["priority"], Value::Integer(3));
        assert_eq!(parsed["done"], Value::Boolean(true));
        assert_eq!(parsed["ratio"], Value::Float(0.5));
    }

    /// A null JS value should map to `Value::Null`, not deserialize-fail.
    #[test]
    fn execute_operation_params_accepts_null() {
        let parsed: HashMap<String, Value> =
            serde_json::from_str(r#"{"content": null}"#).expect("parse params_json");
        assert_eq!(parsed["content"], Value::Null);
    }

    /// Nested objects are valid — they come through as `Value::Object`.
    #[test]
    fn execute_operation_params_accepts_nested_object() {
        let parsed: HashMap<String, Value> =
            serde_json::from_str(r#"{"meta": {"k": "v"}}"#).expect("parse params_json");
        match &parsed["meta"] {
            Value::Object(m) => assert_eq!(m["k"], Value::String("v".into())),
            other => panic!("expected Value::Object, got {other:?}"),
        }
    }
}
