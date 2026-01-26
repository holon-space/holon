//! FFI bridge functions for Flutter
//!
//! This module provides a minimal FFI surface exposing only FrontendSession and essential types.
//! Low-level holon_prql_render types (Expr, ModuleDef, Lineage) are hidden as implementation details.
//!
//! ## Architecture: FrontendSession as the Primary API
//!
//! All query and operation methods are accessed through `FrontendSession`, not `BackendEngine`.
//! This design guarantees that:
//!
//! 1. **No initialization race conditions**: `FrontendSession::new()` completes all schema
//!    initialization (including materialized views like `block_with_path`) before returning.
//!    Since all query methods are on `FrontendSession`, they can only be called after init.
//!
//! 2. **Identical code paths**: Flutter and E2E tests use the exact same API surface
//!    (`FrontendSession`), eliminating bugs that only appear in production.
//!
//! 3. **Type-safe ordering**: It's impossible to call `initial_widget()` before
//!    `init_render_engine()` completes - the compiler enforces this.

use crate::api::types::TraceContext;
use crate::frb_generated::StreamSink;
use flutter_rust_bridge::frb;
use holon_api::render_types::{BinaryOperator, RenderExpr};
pub use holon_api::{BatchMapChange, BatchMapChangeWithMetadata, MapChange};
use holon_api::{EntityName, OperationDescriptor, ProviderAuthStatus, QueryLanguage, Value};

/// The root layout block ID (EntityUri string, e.g. "block:root-layout").
/// Single source of truth — Flutter should use this instead of hardcoding.
pub fn root_layout_block_id() -> String {
    holon_api::ROOT_LAYOUT_BLOCK_ID.to_string()
}
use holon_frontend::FrontendSession;
use once_cell::sync::OnceCell;
use opentelemetry::global;
use opentelemetry::trace::{Span, Tracer};
use opentelemetry::Context;
use std::collections::HashMap;
use std::sync::Arc;
use tokio_stream::StreamExt;

// Re-export types needed by generated code
pub use holon_api::Change;

// Global singleton to store the session (NOT the engine directly)
// This prevents Flutter Rust Bridge from disposing it during async operations
// and guarantees all methods are called after initialization completes.
static GLOBAL_SESSION: OnceCell<Arc<FrontendSession>> = OnceCell::new();

/// Create a default EnvFilter that suppresses noisy HTTP client and OpenTelemetry logs
fn default_env_filter() -> tracing_subscriber::EnvFilter {
    // Some crates use dashes in target names, others use underscores - filter both variants
    tracing_subscriber::EnvFilter::new(
        "info,\
         reqwest=warn,\
         hyper=warn,\
         hyper_util=warn,\
         h2=warn,\
         tower=warn,\
         opentelemetry=warn,\
         opentelemetry_sdk=warn,\
         opentelemetry_http=warn,\
         opentelemetry_otlp=warn,\
         opentelemetry-sdk=warn,\
         opentelemetry-http=warn,\
         opentelemetry-otlp=warn,\
         holon=debug,\
         rust_lib_holon=debug",
    )
}

/// Create an OpenTelemetry span from optional trace context
///
/// If trace_context is provided, creates a child span. Otherwise creates a new root span.
fn create_span_from_context(
    name: &'static str,
    trace_context: Option<TraceContext>,
) -> impl opentelemetry::trace::Span {
    // Use service name from env or default - convert to static string
    let service_name =
        std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "holon-backend".to_string());
    let service_name_static: &'static str = Box::leak(service_name.into_boxed_str());
    let tracer = global::tracer(service_name_static);

    if let Some(ctx) = trace_context {
        if let Some(span_ctx) = ctx.to_span_context() {
            // Create child span from provided context
            // Use Context::current() and attach span context
            use opentelemetry::trace::TraceContextExt;
            let parent_ctx = Context::current().with_remote_span_context(span_ctx);
            tracer.start_with_context(name, &parent_ctx)
        } else {
            // Invalid context, create new root span
            tracer.start(name)
        }
    } else {
        // No context provided, create new root span
        tracer.start(name)
    }
}

use holon::storage::turso::RowChangeStream;

/// Spawn a task to forward CDC stream events to a Flutter sink.
///
/// Enriches Created/Updated rows with computed fields before forwarding.
/// Uses the shared `enrich_batch` from `ui_watcher`.
fn spawn_stream_forwarder(
    mut stream: RowChangeStream,
    sink: MapChangeSink,
    profile_resolver: std::sync::Arc<dyn holon::entity_profile::ProfileResolving>,
) {
    tokio::spawn(async move {
        use tracing::{debug, info, warn};

        info!("[FFI] Stream forwarding task started");
        while let Some(batch_with_metadata) = stream.next().await {
            let metadata = batch_with_metadata.metadata.clone();

            let enriched_changes = holon::api::ui_watcher::enrich_batch(
                batch_with_metadata.inner.items,
                &profile_resolver,
            );
            let map_changes: Vec<MapChange> = enriched_changes
                .into_iter()
                .map(|c| c.map(|e| e.into_inner()))
                .collect();

            let batch_map_change_with_metadata = BatchMapChangeWithMetadata {
                inner: BatchMapChange { items: map_changes },
                metadata,
            };

            if sink.sink.add(batch_map_change_with_metadata).is_err() {
                warn!("[FFI] Sink closed, stopping stream forwarding");
                break;
            }
            debug!("[FFI] Forwarded batch to sink");
        }
        warn!("[FFI] CDC stream ended — sink will be dropped, signaling stream end to Flutter");
        drop(sink);
    });
}

/// Initialize OpenTelemetry tracing and logging
///
/// Sets up OTLP and stdout exporters based on environment variables.
/// Bridges tracing to OpenTelemetry so existing tracing spans appear in traces.
/// Also bridges tracing logs to OpenTelemetry logs for log export.
async fn init_opentelemetry() -> anyhow::Result<()> {
    use opentelemetry::global;
    use opentelemetry::KeyValue;
    use opentelemetry_sdk::Resource;
    use std::env;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::Registry;

    // Get service name from env or use default
    let service_name =
        env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "holon-backend".to_string());

    // Determine which exporters to use
    let exporter_type = env::var("OTEL_TRACES_EXPORTER").unwrap_or_else(|_| "none".to_string());

    if exporter_type == "none" {
        let subscriber = Registry::default()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .with_ansi(false),
            )
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| default_env_filter()),
            );
        let _ = tracing::subscriber::set_global_default(subscriber);
        tracing::debug!(
            "[FFI] Tracing initialized without OpenTelemetry (set OTEL_TRACES_EXPORTER to enable)"
        );
        return Ok(());
    }

    // Create resource with service name
    let resource = Resource::builder_empty()
        .with_attributes(vec![KeyValue::new("service.name", service_name.clone())])
        .build();

    // Set up trace provider and log provider
    if exporter_type.contains("otlp") {
        let otlp_endpoint = env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
            .unwrap_or_else(|_| "http://localhost:4318".to_string());

        // Remove trailing slash if present
        let base_endpoint = otlp_endpoint.trim_end_matches('/').to_string();

        // Traces endpoint: /v1/traces
        let traces_endpoint = format!("{}/v1/traces", base_endpoint);
        // Logs endpoint: /v1/logs
        let logs_endpoint = format!("{}/v1/logs", base_endpoint);

        tracing::debug!("[FFI] Initializing OpenTelemetry OTLP exporters:");
        tracing::debug!("[FFI]   Traces endpoint: {}", traces_endpoint);
        tracing::debug!("[FFI]   Logs endpoint: {}", logs_endpoint);

        // Use OTLP exporter builder (0.31 API)
        use opentelemetry_otlp::WithExportConfig;

        // Create OTLP trace exporter using builder - use with_http() to set HTTP protocol
        let trace_exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(traces_endpoint)
            .build()?;

        // Create OTLP log exporter
        let log_exporter = opentelemetry_otlp::LogExporter::builder()
            .with_http()
            .with_endpoint(logs_endpoint)
            .build()?;

        // Set up trace provider
        use opentelemetry_sdk::trace::SdkTracerProvider;
        let tracer_provider = SdkTracerProvider::builder()
            .with_batch_exporter(trace_exporter)
            .with_resource(resource.clone())
            .build();

        global::set_tracer_provider(tracer_provider);

        // Set up log provider
        use opentelemetry_sdk::logs::SdkLoggerProvider;
        let logger_provider = SdkLoggerProvider::builder()
            .with_batch_exporter(log_exporter)
            .with_resource(resource.clone())
            .build();

        // Convert service_name to static string for tracer
        let service_name_static: &'static str = Box::leak(service_name.clone().into_boxed_str());
        let tracer = global::tracer(service_name_static);

        // Bridge tracing spans to OpenTelemetry traces
        let telemetry_layer = tracing_opentelemetry::OpenTelemetryLayer::new(tracer);

        // Bridge tracing logs to OpenTelemetry logs
        // Filter to only include actual log events (info!, debug!, warn!, error!), not span lifecycle events
        use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
        use tracing_subscriber::filter::{FilterFn, Filtered};

        // Only process events (not spans) - span lifecycle events are handled by telemetry_layer
        let log_filter = FilterFn::new(|metadata| {
            // Only process events, not spans
            metadata.is_event()
        });

        let log_bridge = Filtered::new(
            OpenTelemetryTracingBridge::new(&logger_provider),
            log_filter,
        );

        // Combine with existing fmt layer
        let subscriber = Registry::default()
            .with(telemetry_layer)
            .with(log_bridge)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .with_ansi(false),
            )
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| default_env_filter()),
            );

        // Initialize subscriber (idempotent)
        let _ = tracing::subscriber::set_global_default(subscriber);

        tracing::debug!("[FFI] OpenTelemetry tracing and logging initialized with OTLP exporters");
    } else {
        // Use stdout exporters only
        use opentelemetry_stdout::{LogExporter, SpanExporter};
        let stdout_trace_exporter = SpanExporter::default();
        let stdout_log_exporter = LogExporter::default();

        tracing::debug!("[FFI] Initializing OpenTelemetry stdout exporters");

        // Set up trace provider
        use opentelemetry_sdk::trace::SdkTracerProvider;
        let tracer_provider = SdkTracerProvider::builder()
            .with_simple_exporter(stdout_trace_exporter)
            .with_resource(resource.clone())
            .build();

        global::set_tracer_provider(tracer_provider);

        // Set up log provider
        use opentelemetry_sdk::logs::SdkLoggerProvider;
        let logger_provider = SdkLoggerProvider::builder()
            .with_simple_exporter(stdout_log_exporter)
            .with_resource(resource)
            .build();

        // Bridge tracing spans to OpenTelemetry traces
        // Convert service_name to static string for tracer
        let service_name_static: &'static str = Box::leak(service_name.clone().into_boxed_str());
        let tracer = global::tracer(service_name_static);
        let telemetry_layer = tracing_opentelemetry::OpenTelemetryLayer::new(tracer);

        // Bridge tracing logs to OpenTelemetry logs
        // Filter to only include actual log events (info!, debug!, warn!, error!), not span lifecycle events
        use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
        use tracing_subscriber::filter::{FilterFn, Filtered};

        // Only process events (not spans) - span lifecycle events are handled by telemetry_layer
        let log_filter = FilterFn::new(|metadata| {
            // Only process events, not spans
            metadata.is_event()
        });

        let log_bridge = Filtered::new(
            OpenTelemetryTracingBridge::new(&logger_provider),
            log_filter,
        );

        // Combine with existing fmt layer
        let subscriber = Registry::default()
            .with(telemetry_layer)
            .with(log_bridge)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .with_ansi(false),
            )
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| default_env_filter()),
            );

        // Initialize subscriber (idempotent)
        let _ = tracing::subscriber::set_global_default(subscriber);

        tracing::debug!("[FFI] OpenTelemetry tracing and logging initialized with stdout exporters");
    }

    Ok(())
}

/// Initialize the render engine with a database at the given path
///
/// This is the main entry point for Flutter. It creates a `FrontendSession` which:
/// 1. Initializes the database and schema (including materialized views)
/// 2. Waits for file watcher readiness (if OrgMode is configured)
/// 3. Detects any startup errors
///
/// The session is stored globally so that subsequent FFI calls can access it.
/// All query methods go through `FrontendSession`, ensuring they can only be
/// called after initialization completes.
///
/// # Parameters
/// * `db_path` - Path to the database file
/// * `config` - Configuration map containing:
///   - `TODOIST_API_KEY` - Todoist API key for sync
///   - `ORGMODE_ROOT_DIRECTORY` - OrgMode files root directory
///   - `MCP_SERVER_PORT` - Port for MCP HTTP server (optional, starts server if provided)
///
/// # Returns
/// An opaque handle to the session. The actual `FrontendSession` is stored globally
/// and accessed by subsequent FFI calls. This return value is kept alive by Flutter
/// to prevent the Rust Bridge from disposing the underlying resources.
pub async fn init_render_engine(
    db_path: String,
    config: HashMap<String, String>,
) -> anyhow::Result<ArcFrontendSession> {
    use std::collections::HashSet;
    use std::println;

    // Hot restart: if session already exists, return it.
    // Flutter hot restart re-runs main() but Rust statics survive,
    // so the existing session (DB, matviews, file watchers) is still valid.
    if let Some(existing) = GLOBAL_SESSION.get() {
        println!(
            "[FFI] Session already initialized (hot restart detected), reusing existing session"
        );
        return Ok(ArcFrontendSession(existing.clone()));
    }

    // Initialize OpenTelemetry (includes tracing subscriber with OpenTelemetry bridge)
    init_opentelemetry().await?;

    println!("[FFI] Tracing subscriber initialized - Rust logs will appear below");
    tracing::debug!("[FFI] Tracing subscriber initialized - Rust logs will appear below");

    // Build HolonConfig from HashMap
    let mut holon_config = holon_frontend::HolonConfig::default();
    holon_config.db_path = Some(db_path.into());

    if let Some(root_dir) = config.get("ORGMODE_ROOT_DIRECTORY") {
        println!("[FFI] Configuring OrgMode with root directory: {}", root_dir);
        holon_config.orgmode.root_directory = Some(root_dir.into());
    }

    let loro_enabled = config
        .get("LORO_ENABLED")
        .map(|v| !v.is_empty() && v != "0" && v.to_lowercase() != "false")
        .unwrap_or(false);
    if loro_enabled {
        println!("[FFI] Enabling Loro CRDT layer");
        holon_config.loro.enabled = Some(true);
    }

    if let Some(api_key) = config.get("TODOIST_API_KEY") {
        println!("[FFI] Configuring Todoist with API key");
        holon_config.todoist.api_key = Some(api_key.clone());
    }

    // Build UiInfo from config
    let ui_info = if let Some(widgets_csv) = config.get("AVAILABLE_WIDGETS") {
        let widgets: std::collections::HashSet<String> = widgets_csv
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        holon_api::UiInfo {
            available_widgets: widgets,
            screen_size: None,
        }
    } else {
        holon_api::UiInfo::permissive()
    };

    let config_dir = holon_frontend::config::resolve_config_dir(holon_config.config_dir.as_deref());
    let session_config = holon_frontend::SessionConfig::new(ui_info);

    // Create session using FrontendSession (waits for readiness!)
    println!("[FFI] Creating FrontendSession...");
    let session =
        FrontendSession::new_from_config(holon_config, session_config, config_dir, HashSet::new())
            .await?;

    // Check for startup errors (DDL/sync races)
    if session.has_startup_errors() {
        tracing::debug!(
            "[FFI] WARNING: {} startup errors detected during initialization",
            session.startup_error_count()
        );
    }

    println!("[FFI] FrontendSession created successfully");

    // Store in global singleton
    GLOBAL_SESSION
        .set(session.clone())
        .map_err(|_| anyhow::anyhow!("Session already initialized"))?;

    // Start MCP server (only on non-WASM targets)
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mcp_port: u16 = config
            .get("MCP_SERVER_PORT")
            .and_then(|s| s.parse().ok()) // ALLOW(ok): non-critical env var parse
            .unwrap_or(8520);
        holon_mcp::di::start_embedded_mcp_server(Some(session.engine().clone()), None, mcp_port);
    }

    Ok(ArcFrontendSession(session))
}

/// Opaque handle to the FrontendSession for Flutter
///
/// This is returned from `init_render_engine` and should be kept alive by Flutter
/// to prevent the Rust Bridge from disposing the underlying session.
/// All actual operations go through the global `GLOBAL_SESSION`.
#[frb(opaque)]
pub struct ArcFrontendSession(Arc<FrontendSession>);

/// Helper to get the global session, returning a clear error if not initialized
pub(crate) fn get_session() -> anyhow::Result<Arc<FrontendSession>> {
    GLOBAL_SESSION
        .get()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Session not initialized. Call init_render_engine first."))
}

/// flutter_rust_bridge:non_opaque
pub struct MapChangeSink {
    pub sink: StreamSink<BatchMapChangeWithMetadata>,
}

/// Sink for UiEvent stream (required by FRB — can't use type alias for StreamSink<T>)
///
/// flutter_rust_bridge:non_opaque
pub struct UiEventSink {
    pub sink: StreamSink<holon_api::UiEvent>,
}

/// Handle for controlling a watch_ui stream (variant switching).
///
/// Holds the command sender from a `WatchHandle`. The event receiver is
/// forwarded to a Flutter `StreamSink` in a spawned task.
#[frb(opaque)]
pub struct FfiWatchHandle {
    command_tx: tokio::sync::mpsc::Sender<holon_api::WatcherCommand>,
}

/// Watch a block's UI with automatic error recovery and structural hot-swap.
///
/// Returns a `FfiWatchHandle` for variant switching. UiEvents (Structure + Data)
/// are sent to the provided sink. Unlike `render_entity`, errors become
/// `UiEvent::Structure` events with inline error RenderExprs — the stream
/// stays open and recovers when the block is fixed.
pub async fn watch_ui(
    block_id: String,
    sink: UiEventSink,
) -> anyhow::Result<FfiWatchHandle> {
    let session = get_session()?;

    let block_uri = holon_api::EntityUri::parse(&block_id)
        .map_err(|e| anyhow::anyhow!("Invalid block_id '{}': {}", block_id, e))?;
    let watch = session
        .watch_ui(&block_uri)
        .await?;

    let (mut output_rx, command_tx) = watch.into_parts();

    // Forward UiEvents from the mpsc receiver to the Flutter StreamSink
    tokio::spawn(async move {
        while let Some(event) = output_rx.recv().await {
            if sink.sink.add(event).is_err() {
                tracing::warn!("[FFI] UiEvent sink closed for block '{}'", block_id);
                break;
            }
        }
        drop(sink);
    });

    Ok(FfiWatchHandle { command_tx })
}

/// Switch the entity profile variant for an active watch_ui stream.
pub async fn set_variant(handle: &FfiWatchHandle, variant: String) -> anyhow::Result<()> {
    handle
        .command_tx
        .send(holon_api::WatcherCommand::SetVariant(variant))
        .await
        .map_err(|_| anyhow::anyhow!("Watch stream closed — cannot set variant"))
}

/// Compile one of the supported query languages, execute it, and set up CDC streaming.
///
/// Returns the initial data rows. CDC deltas are forwarded to the sink.
///
/// # UI Usage
/// The UI should:
/// 1. Subscribe to the MapChangeSink using StreamBuilder in Flutter
/// 2. Key widgets by entity ID from data.get("id"), NOT by rowid
/// 3. Handle Added/Updated/Removed events to update UI
///
pub async fn query_and_watch(
    prql: String,
    params: HashMap<String, Value>,
    sink: MapChangeSink,
    trace_context: Option<TraceContext>,
    context_block_id: Option<String>,
    language: Option<String>,
) -> anyhow::Result<Vec<HashMap<String, Value>>> {
    use holon_frontend::QueryContext;

    let mut span = create_span_from_context("ffi.query_and_watch", trace_context);
    span.set_attribute(opentelemetry::KeyValue::new("prql.query", prql.clone()));

    let session = get_session()?;

    let lang: QueryLanguage = language
        .as_deref()
        .map(|s| s.parse::<QueryLanguage>())
        .transpose()
        .map_err(|e| anyhow::anyhow!("Invalid query language: {e}"))?
        .unwrap_or(QueryLanguage::HolonPrql);

    let context = if let Some(id) = context_block_id {
        let uri = holon_api::EntityUri::parse(&id)
            .map_err(|e| anyhow::anyhow!("Invalid context_block_id '{}': {}", id, e))?;
        let block_path = session.lookup_block_path(&uri).await?;
        let ctx = QueryContext::for_block_with_path(&uri, None, block_path);
        Some(ctx)
    } else {
        let ctx = QueryContext::root();
        Some(ctx)
    };

    let sql = session.engine().compile_to_sql(&prql, lang)?;
    let mut stream = session
        .engine()
        .query_and_watch(sql, params, context)
        .await?;

    // Collect initial data from the first batch (Change::Created items)
    let mut initial_rows: Vec<HashMap<String, Value>> = Vec::new();
    if let Some(first_batch) = stream.next().await {
        for row_change in first_batch.inner.items {
            if let holon_api::Change::Created { data, .. } = row_change.change {
                initial_rows.push(data);
            }
        }
    }

    span.set_attribute(opentelemetry::KeyValue::new(
        "query.result_count",
        initial_rows.len() as i64,
    ));

    spawn_stream_forwarder(
        stream,
        sink,
        session.engine().profile_resolver().clone(),
    );

    span.end();
    Ok(initial_rows)
}

/// Render a block by its ID.
///
/// Given a block ID, finds its query source child, compiles and executes the query,
/// parses any render sibling into a RenderExpr, and returns the RenderExpr.
/// Initial data and CDC deltas are forwarded to the sink.
pub async fn render_entity(
    block_id: String,
    sink: MapChangeSink,
    preferred_variant: Option<String>,
) -> anyhow::Result<RenderExpr> {
    let session = get_session()?;

    let block_uri = holon_api::EntityUri::parse(&block_id)
        .map_err(|e| anyhow::anyhow!("Invalid block_id '{}': {}", block_id, e))?;
    let (render_expr, stream) = session
        .engine()
        .blocks()
        .render_entity(&block_uri, &preferred_variant)
        .await?;

    spawn_stream_forwarder(
        stream,
        sink,
        session.engine().profile_resolver().clone(),
    );

    Ok(render_expr)
}

/// Get available operations for an entity
///
/// Returns a list of operation descriptors available for the given entity_name.
/// Use "*" as entity_name to get wildcard operations.
///
/// # FFI Function
/// This is exposed to Flutter via flutter_rust_bridge
pub async fn available_operations(entity_name: String) -> anyhow::Result<Vec<OperationDescriptor>> {
    let session = get_session()?;
    Ok(session.available_operations(&entity_name).await)
}

/// Execute an operation on the database
///
/// # FFI Function
/// This is exposed to Flutter via flutter_rust_bridge
///
/// Operations mutate the database directly. UI updates happen via CDC streams.
/// This follows the unidirectional data flow: Action → Model → View
///
/// # Note
/// This function does NOT return new data. Changes propagate through:
/// Operation → DB mutation → CDC event → watch_query stream → UI update
pub async fn execute_operation(
    entity_name: String,
    op_name: String,
    params: HashMap<String, Value>,
    trace_context: Option<TraceContext>,
) -> anyhow::Result<Option<String>> {
    use opentelemetry::trace::TraceContextExt;
    use tracing::info;
    use tracing::Instrument;
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    // Create tracing span that will be bridged to OpenTelemetry
    let span = tracing::span!(
        tracing::Level::INFO,
        "ffi.execute_operation",
        "operation.entity" = %entity_name,
        "operation.name" = %op_name
    );

    // Build the parent context from Flutter's trace context
    let parent_ctx = if let Some(ref ctx) = trace_context {
        if let Some(span_ctx) = ctx.to_span_context() {
            Context::current().with_remote_span_context(span_ctx)
        } else {
            Context::current()
        }
    } else {
        Context::current()
    };

    let _ = span.set_parent(parent_ctx.clone());

    // Create BatchTraceContext for task-local propagation
    let batch_trace_ctx = trace_context
        .as_ref()
        .and_then(|ctx| ctx.to_span_context())
        .map(|span_ctx| holon_api::BatchTraceContext::from_span_context(&span_ctx));

    // Use task-local storage to propagate trace context through async call chain
    let result = if let Some(trace_ctx) = batch_trace_ctx {
        holon_api::CURRENT_TRACE_CONTEXT
            .scope(trace_ctx, async {
                let session = get_session()?;

                info!(
                    "[FFI] execute_operation called: entity={}, op={}, params={:?}",
                    entity_name, op_name, params
                );

                session
                    .execute_operation(&EntityName::new(&entity_name), &op_name, params.clone())
                    .await
            })
            .instrument(span)
            .await
    } else {
        async {
            let session = get_session()?;

            info!(
                "[FFI] execute_operation called: entity={}, op={}, params={:?}",
                entity_name, op_name, params
            );

            session
                .execute_operation(&EntityName::new(&entity_name), &op_name, params.clone())
                .await
        }
        .instrument(span)
        .await
    };

    match &result {
        Ok(_) => {
            info!(
                "[FFI] execute_operation succeeded: entity={}, op={}",
                entity_name, op_name
            );
        }
        Err(e) => {
            tracing::error!(
                "[FFI] execute_operation failed: entity={}, op={}, error={}",
                entity_name,
                op_name,
                e
            );
        }
    }

    result
        .map(|opt_value| opt_value.map(|v| v.to_json_string()))
        .map_err(|e| {
            anyhow::anyhow!(
                "Operation '{}' on entity '{}' failed: {}",
                op_name,
                entity_name,
                e
            )
        })
}

/// Look up a render sibling block for a given parent block ID.
///
/// Previously found a block with `source_language = "render"` that shares the same parent_id.
/// TODO: Render sibling lookup was removed with PRQL render directive removal.
/// This stub remains for FFI compatibility.
pub async fn lookup_render_sibling(_parent_id: String) -> anyhow::Result<Option<String>> {
    Ok(None)
}

/// Check if an operation is available for an entity
///
/// # FFI Function
/// This is exposed to Flutter via flutter_rust_bridge
///
/// # Returns
/// `true` if the operation is available, `false` otherwise
pub async fn has_operation(entity_name: String, op_name: String) -> anyhow::Result<bool> {
    let session = get_session()?;
    Ok(session.has_operation(&entity_name, &op_name).await)
}

/// Undo the last operation
///
/// Executes the inverse operation from the undo stack and pushes it to the redo stack.
/// Returns true if an operation was undone, false if the undo stack is empty.
pub async fn undo() -> anyhow::Result<bool> {
    let session = get_session()?;
    session.undo().await
}

/// Redo the last undone operation
///
/// Executes the inverse of the last undone operation and pushes it back to the undo stack.
/// Returns true if an operation was redone, false if the redo stack is empty.
pub async fn redo() -> anyhow::Result<bool> {
    let session = get_session()?;
    session.redo().await
}

/// Check if undo is available
pub async fn can_undo() -> anyhow::Result<bool> {
    let session = get_session()?;
    Ok(session.can_undo().await)
}

/// Check if redo is available
pub async fn can_redo() -> anyhow::Result<bool> {
    let session = get_session()?;
    Ok(session.can_redo().await)
}

/// Get the initial widget for the application root
///
/// Get authentication status for all configured MCP providers.
///
/// Returns a list of `ProviderAuthStatus` indicating whether each provider
/// is authenticated, needs OAuth consent, or has a failed auth state.
/// Frontends should poll this after initialization and display appropriate UI.
pub async fn get_provider_auth_statuses() -> anyhow::Result<Vec<ProviderAuthStatus>> {
    // TODO: Wire to FrontendSession once provider registry tracks OAuth providers.
    // For now, return empty — no OAuth providers configured yet.
    Ok(vec![])
}

/// Complete an OAuth consent flow after the user authorized in the browser.
///
/// The frontend captures the OAuth redirect callback URL (via flutter_web_auth_2)
/// and extracts the `code` and `state` query parameters, then passes them here.
/// Rust exchanges the code for a token and connects to the MCP server.
///
/// # Parameters
/// * `provider_name` - The provider identifier (MCP server URI) from `NeedsConsent`
/// * `code` - The authorization code from the OAuth callback `?code=...`
/// * `state` - The CSRF state from the OAuth callback `?state=...`
pub async fn complete_provider_oauth(
    provider_name: String,
    code: String,
    state: String,
) -> anyhow::Result<()> {
    let _ = (provider_name, code, state);
    // TODO: Wire to FrontendSession's PendingOAuthFlows.complete_oauth().
    // The PendingOAuthFlows registry will be held on FrontendSession once
    // OAuth provider configuration is added.
    Ok(())
}

// ──── ViewModel interpretation (shadow DOM) ────

/// Interpret a RenderExpr with data rows into a ViewModel tree and return it as JSON.
///
/// This is the same interpretation that GPUI does inline (shadow_interp.interpret).
/// Flutter calls this after receiving a UiEvent::Structure to get the pre-built
/// widget tree, avoiding the need for Dart-side RenderInterpreter.
///
/// The returned JSON follows the serde serialization of `ViewModel` with
/// `ViewKind` tagged as `{"widget": "text", "content": "...", ...}`.
///
/// Also available as `interpret_render_expr` (identical signature).
pub fn interpret_render_expr(
    render_expr: RenderExpr,
    rows: Vec<HashMap<String, Value>>,
) -> anyhow::Result<String> {
    use holon_frontend::reactive::BuilderServices;
    let session = get_session()?;
    let services = holon_frontend::reactive::HeadlessBuilderServices::new(session.engine().clone());
    let rows: Vec<std::sync::Arc<HashMap<String, Value>>> = rows.into_iter().map(std::sync::Arc::new).collect();
    let render_ctx = holon_frontend::RenderContext::default().with_data_rows(rows);
    let view_model = services.interpret(&render_expr, &render_ctx).snapshot();

    serde_json::to_string(&view_model)
        .map_err(|e| anyhow::anyhow!("Failed to serialize ViewModel: {e}"))
}

// ──── Render evaluation (shared with Blinc/WaterUI/Dioxus) ────

/// Evaluate a RenderExpr to a Value given a row of data.
///
/// This is the same evaluation logic used by all Rust frontends.
/// Flutter calls this instead of reimplementing evaluation in Dart.
pub fn eval_render_expr(expr: RenderExpr, row: HashMap<String, Value>) -> Value {
    holon_api::render_eval::eval_to_value(&expr, &row)
}

/// Evaluate a binary operation on two Values.
pub fn eval_binary(op: BinaryOperator, left: Value, right: Value) -> Value {
    holon_api::render_eval::eval_binary_op(&op, &left, &right)
}

/// Convert a Value to its display string representation.
pub fn value_display_string(value: Value) -> String {
    value.to_display_string()
}

// ──── Widget state (persisted sidebar open/close, width) ────

/// Check if a widget is open (default: true if not stored).
pub fn is_widget_open(block_id: String) -> anyhow::Result<bool> {
    let session = get_session()?;
    Ok(session.widget_state(&block_id).open)
}

/// Set a widget's open/close state and persist to disk.
pub fn set_widget_open(block_id: String, open: bool) -> anyhow::Result<()> {
    let session = get_session()?;
    session.set_widget_open(&block_id, open);
    Ok(())
}

// ──── Preferences ────

/// Preference render data returned to Flutter.
///
/// Contains the RenderExpr tree (section > pref_field) and data rows
/// with current values. Flutter renders this with its RenderInterpreter.
///
/// flutter_rust_bridge:non_opaque
pub struct PreferencesRenderData {
    pub render_expr: RenderExpr,
    pub rows: Vec<HashMap<String, Value>>,
}

/// Get the preferences UI render data.
///
/// Returns a RenderExpr tree and data rows describing all preference fields.
/// Flutter renders this with its existing RenderInterpreter / BuilderRegistry.
pub fn get_preferences_render() -> anyhow::Result<PreferencesRenderData> {
    let session = get_session()?;
    let (render_expr, rows) = session.preferences_render_data();
    let rows = rows.into_iter().map(|arc| (*arc).clone()).collect();
    Ok(PreferencesRenderData { render_expr, rows })
}

/// Read a preference value by key.
///
/// Returns the current value as a JSON string (or default if not set).
/// Keys use dotted notation, e.g. "ui.theme", "todoist.api_key".
pub fn get_preference(key: String) -> anyhow::Result<String> {
    let session = get_session()?;
    let pref_key = holon_frontend::PrefKey::new(&key);
    let value = session.get_preference(&pref_key);
    Ok(value.to_string())
}

/// Set a preference value by key and persist to disk.
///
/// The value is a plain string — Rust parses it into the appropriate type
/// based on the preference definition. Keys use dotted notation.
pub fn set_preference(key: String, value: String) -> anyhow::Result<()> {
    let session = get_session()?;
    let pref_key = holon_frontend::PrefKey::new(&key);
    let toml_value = toml::Value::String(value);
    session.set_preference(&pref_key, toml_value);
    Ok(())
}

/// Set a boolean preference value by key and persist to disk.
pub fn set_preference_bool(key: String, value: bool) -> anyhow::Result<()> {
    let session = get_session()?;
    let pref_key = holon_frontend::PrefKey::new(&key);
    session.set_preference(&pref_key, toml::Value::Boolean(value));
    Ok(())
}

// `run_shared_pbt` is in shared_pbt.rs (separate file required for FRB DartFnFuture codegen)

// ──── PBT Engine routing ────
// The shared PBT creates its own BackendEngine (with its own temp dir, database, etc.).
// The Dart callback calls `pbt_execute_operation` which routes to this engine
// instead of the production GLOBAL_SESSION.
static PBT_ENGINE: std::sync::RwLock<Option<Arc<holon::api::BackendEngine>>> =
    std::sync::RwLock::new(None);

pub(crate) fn set_pbt_engine(engine: Arc<holon::api::BackendEngine>) {
    *PBT_ENGINE.write().unwrap() = Some(engine);
}

pub(crate) fn clear_pbt_engine() {
    *PBT_ENGINE.write().unwrap() = None;
}

/// Install the PBT's BackendEngine as GLOBAL_SESSION.
///
/// Called during `pbt_setup` after StartApp completes. Wraps the PBT engine
/// in a FrontendSession and sets it as GLOBAL_SESSION. When Flutter's
/// `init_render_engine` is subsequently called, it detects the existing
/// session (hot restart path) and reuses it — so the Flutter app and PBT
/// share the same database.
pub fn install_pbt_as_global_session() -> anyhow::Result<()> {
    let engine = PBT_ENGINE
        .read()
        .unwrap()
        .clone()
        .ok_or_else(|| anyhow::anyhow!("PBT engine not installed"))?;
    let session = Arc::new(FrontendSession::from_engine(engine));
    GLOBAL_SESSION
        .set(session)
        .map_err(|_| anyhow::anyhow!("GLOBAL_SESSION already set — cannot install PBT session"))?;
    Ok(())
}

/// Execute an operation on the PBT's BackendEngine.
///
/// Called by the Dart callback during `run_shared_pbt`. Routes mutations to the
/// PBT's own engine (separate from the production GLOBAL_SESSION), ensuring the
/// PBT's TestEnvironment sees the mutations.
pub async fn pbt_execute_operation(
    entity_name: String,
    op_name: String,
    params: HashMap<String, Value>,
) -> anyhow::Result<()> {
    let engine =
        PBT_ENGINE.read().unwrap().clone().ok_or_else(|| {
            anyhow::anyhow!("PBT engine not initialized — run_shared_pbt not active")
        })?;
    engine
        .execute_operation(&EntityName::new(&entity_name), &op_name, params)
        .await?;
    Ok(())
}
