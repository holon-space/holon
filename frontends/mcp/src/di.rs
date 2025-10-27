//! Dependency Injection module for MCP server
//!
//! This module provides DI integration for embedding the MCP server within
//! applications that use Ferrous DI. The MCP server shares the same BackendEngine
//! instance as the host application, enabling shared undo/redo, CDC streams, and operations.
//!
//! # Usage
//!
//! ```rust,ignore
//! use holon_mcp::di::McpInjectorExt;
//!
//! let engine = holon::di::create_backend_engine(db_path, |services| {
//!     // Register MCP server on port 8000
//!     services.add_mcp_server(8000)?;
//!     Ok(())
//! }).await?;
//!
//! // Start the server
//! let mcp_handle = provider.get_required::<McpServerHandle>();
//! mcp_handle.start().await?;
//! ```

use std::net::SocketAddr;
use std::sync::Arc;

use fluxdi::{Injector, Module, Provider, Shared};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use holon::api::backend_engine::BackendEngine;
use holon_frontend::reactive::BuilderServices;

use crate::server::{DebugServices, HolonMcpServer};

/// Register `DebugServices` as a DI singleton.
/// Call from the frontend's DI setup closure so both the MCP server
/// and the frontend resolve the same instance.
pub fn register_debug_services(injector: &Injector) {
    injector.provide::<DebugServices>(Provider::root(|_| Shared::new(DebugServices::default())));
}

/// Configuration for the MCP server
#[derive(Clone, Debug)]
pub struct McpServerConfig {
    /// Address to bind the HTTP server to
    pub bind_address: SocketAddr,
}

impl McpServerConfig {
    /// Create a new MCP server configuration
    pub fn new(port: u16) -> Self {
        Self {
            bind_address: ([127, 0, 0, 1], port).into(),
        }
    }

    /// Create configuration with a custom bind address
    pub fn with_address(bind_address: SocketAddr) -> Self {
        Self { bind_address }
    }
}

/// Handle for managing MCP server lifecycle
///
/// This struct provides methods to start and stop the MCP HTTP server.
/// The server shares the same BackendEngine as the host application.
pub struct McpServerHandle {
    config: McpServerConfig,
    engine: Option<Arc<BackendEngine>>,
    debug: Arc<DebugServices>,
    builder_services: std::sync::OnceLock<Arc<dyn BuilderServices>>,
    state: Mutex<ServerState>,
}

struct ServerState {
    task: Option<JoinHandle<()>>,
    cancellation_token: Option<CancellationToken>,
}

impl McpServerHandle {
    /// Create a new MCP server handle
    pub fn new(
        config: McpServerConfig,
        engine: Option<Arc<BackendEngine>>,
        debug: Arc<DebugServices>,
        builder_services: Option<Arc<dyn BuilderServices>>,
    ) -> Self {
        let lock = std::sync::OnceLock::new();
        if let Some(bs) = builder_services {
            lock.set(bs).ok();
        }
        Self {
            config,
            engine,
            debug,
            builder_services: lock,
            state: Mutex::new(ServerState {
                task: None,
                cancellation_token: None,
            }),
        }
    }

    /// Set the builder services used for `describe_ui` and related MCP tools.
    ///
    /// Call this after the `BuilderServicesSlot` has been populated (i.e. after
    /// `ReactiveEngine` is available) and before calling [`start`].
    pub fn set_builder_services(&self, services: Arc<dyn BuilderServices>) {
        self.builder_services.set(services).ok();
    }

    /// Start the MCP HTTP server
    ///
    /// Returns an error if the server is already running.
    pub async fn start(&self) -> anyhow::Result<()> {
        let mut state = self.state.lock().await;

        if state.task.is_some() {
            anyhow::bail!("MCP server is already running");
        }

        let engine = self.engine.clone();
        let debug = self.debug.clone();
        let builder_services = self.builder_services.get().cloned();
        let bind_address = self.config.bind_address;
        let cancellation_token = CancellationToken::new();
        let token_for_task = cancellation_token.clone();

        let task = tokio::spawn(async move {
            if let Err(e) = run_http_server(
                engine,
                debug,
                builder_services,
                bind_address,
                token_for_task,
            )
            .await
            {
                tracing::error!("MCP server error: {}", e);
            }
        });

        state.task = Some(task);
        state.cancellation_token = Some(cancellation_token);

        tracing::info!("MCP server started on http://{}", self.config.bind_address);
        Ok(())
    }

    /// Stop the MCP HTTP server
    ///
    /// Returns Ok(()) if the server was stopped or wasn't running.
    pub async fn stop(&self) -> anyhow::Result<()> {
        let mut state = self.state.lock().await;

        if let Some(token) = state.cancellation_token.take() {
            token.cancel();
        }

        if let Some(task) = state.task.take() {
            // Wait for the task to finish with a timeout
            match tokio::time::timeout(std::time::Duration::from_secs(5), task).await {
                Ok(Ok(())) => tracing::info!("MCP server stopped gracefully"),
                Ok(Err(e)) => tracing::warn!("MCP server task panicked: {}", e),
                Err(_) => tracing::warn!("MCP server stop timed out"),
            }
        }

        Ok(())
    }

    /// Check if the server is running
    pub async fn is_running(&self) -> bool {
        let state = self.state.lock().await;
        state.task.is_some()
    }

    /// Get the bind address
    pub fn bind_address(&self) -> SocketAddr {
        self.config.bind_address
    }
}

/// Run the MCP HTTP server
///
/// This is the core server loop, extracted for reuse by both the standalone binary
/// and the DI-managed server handle.
///
/// When `HOLON_BROWSER_RELAY_URL` is set, tool calls are forwarded to the browser
/// via the serve.mjs WebSocket hub instead of being handled by a local engine.
pub async fn run_http_server(
    engine: Option<Arc<BackendEngine>>,
    debug: Arc<DebugServices>,
    builder_services: Option<Arc<dyn BuilderServices>>,
    bind_address: SocketAddr,
    cancellation_token: CancellationToken,
) -> anyhow::Result<()> {
    use axum::{response::Html, routing::get, Router};
    use rmcp::transport::{
        streamable_http_server::{
            session::local::LocalSessionManager, tower::StreamableHttpService,
        },
        StreamableHttpServerConfig,
    };

    let cancellation_token_for_service = cancellation_token.clone();

    // Check for browser relay mode.
    if let Ok(hub_url) = std::env::var("HOLON_BROWSER_RELAY_URL") {
        use crate::browser_relay::{BrowserRelay, BrowserRelayServer};

        tracing::info!("[mcp] browser relay mode — hub: {}", hub_url);
        let relay = BrowserRelay::start(hub_url);

        let mcp_service: StreamableHttpService<BrowserRelayServer, LocalSessionManager> =
            StreamableHttpService::new(
                move || Ok(BrowserRelayServer::new(relay.clone())),
                LocalSessionManager::default().into(),
                StreamableHttpServerConfig {
                    sse_keep_alive: Some(std::time::Duration::from_secs(15)),
                    stateful_mode: true,
                    cancellation_token: cancellation_token_for_service,
                },
            );

        let app = axum::Router::new()
            .route("/health", axum::routing::get(|| async { "OK" }))
            .nest_service("/mcp", mcp_service);

        let listener = tokio::net::TcpListener::bind(bind_address).await?;
        tracing::info!("[mcp] browser relay HTTP listening on http://{}", bind_address);

        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                cancellation_token.cancelled().await;
                tracing::info!("MCP relay server shutting down…");
            })
            .await?;

        return Ok(());
    }

    // Create streamable HTTP service
    let mcp_service: StreamableHttpService<HolonMcpServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || {
                Ok(HolonMcpServer::new(
                    engine.clone(),
                    debug.clone(),
                    builder_services.clone(),
                ))
            },
            LocalSessionManager::default().into(),
            StreamableHttpServerConfig {
                sse_keep_alive: Some(std::time::Duration::from_secs(15)),
                stateful_mode: true,
                cancellation_token: cancellation_token_for_service,
            },
        );

    // Create index page
    const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <title>Holon MCP Server</title>
    <style>
        body { font-family: sans-serif; max-width: 800px; margin: 50px auto; padding: 20px; }
        code { background: #f4f4f4; padding: 2px 6px; border-radius: 3px; }
        pre { background: #f4f4f4; padding: 15px; border-radius: 5px; overflow-x: auto; }
    </style>
</head>
<body>
    <h1>Holon MCP Server</h1>
    <p>Model Context Protocol server for Holon backend engine.</p>

    <h2>MCP Endpoint</h2>
    <p>The MCP endpoint is available at: <code>/mcp</code></p>

    <h2>Available Tools</h2>
    <ul>
        <li><code>create_table</code> - Create database tables</li>
        <li><code>insert_data</code> - Insert rows into tables</li>
        <li><code>drop_table</code> - Drop tables</li>
        <li><code>execute_prql</code> - Execute PRQL queries</li>
        <li><code>execute_sql</code> - Execute SQL queries</li>
        <li><code>watch_query</code> - Watch queries for changes</li>
        <li><code>poll_changes</code> - Poll for CDC changes</li>
        <li><code>stop_watch</code> - Stop watching a query</li>
        <li><code>execute_operation</code> - Execute entity operations</li>
        <li><code>list_operations</code> - List available operations</li>
        <li><code>undo</code> / <code>redo</code> - Undo/redo operations</li>
        <li><code>can_undo</code> / <code>can_redo</code> - Check undo/redo availability</li>
    </ul>
</body>
</html>"#;

    async fn index() -> Html<&'static str> {
        Html(INDEX_HTML)
    }

    async fn health_check() -> &'static str {
        "OK"
    }

    // Create router
    let app = Router::new()
        .route("/", get(index))
        .route("/health", get(health_check))
        .nest_service("/mcp", mcp_service);

    // Start HTTP server
    let listener = tokio::net::TcpListener::bind(bind_address).await?;
    tracing::info!("Holon MCP HTTP server listening on http://{}", bind_address);

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            cancellation_token.cancelled().await;
            tracing::info!("MCP server shutting down...");
        })
        .await?;

    Ok(())
}

/// DI module for MCP server
///
/// This module registers the MCP server handle as a singleton service.
/// The handle receives the BackendEngine from DI, ensuring it shares
/// the same instance as the host application.
pub struct McpServerModule;

impl Module for McpServerModule {
    fn configure(&self, injector: &Injector) -> std::result::Result<(), fluxdi::Error> {
        injector.provide::<McpServerHandle>(Provider::root(|resolver| {
            let config = resolver.resolve::<McpServerConfig>();
            let engine = resolver.try_resolve::<BackendEngine>().ok(); // ALLOW(ok): optional DI service

            // Resolve DebugServices if registered, otherwise use default
            // ALLOW(ok): optional DI service
            let debug = resolver
                .try_resolve::<DebugServices>()
                .ok()
                .unwrap_or_else(|| Arc::new(DebugServices::default()));

            Shared::new(McpServerHandle::new((*config).clone(), engine, debug, None))
        }));

        Ok(())
    }
}

/// Start an embedded MCP HTTP server for a frontend.
///
/// Reads `MCP_SERVER_PORT` from env (default: `default_port`), registers the
/// MCP server module in DI, resolves the handle, and spawns the server task.
///
/// Replaces the ~15 lines of boilerplate previously duplicated in every frontend.
pub fn start_embedded_mcp_server(
    engine: Option<Arc<BackendEngine>>,
    builder_services: Option<Arc<dyn BuilderServices>>,
    default_port: u16,
) {
    start_embedded_mcp_server_with_debug(
        engine,
        builder_services,
        default_port,
        Arc::new(DebugServices::default()),
    )
}

pub fn start_embedded_mcp_server_with_debug(
    engine: Option<Arc<BackendEngine>>,
    builder_services: Option<Arc<dyn BuilderServices>>,
    default_port: u16,
    debug: Arc<DebugServices>,
) {
    let mcp_port: u16 = std::env::var("MCP_SERVER_PORT")
        .ok() // ALLOW(ok): non-critical env var
        .and_then(|s| s.parse().ok()) // ALLOW(ok): non-critical env var parse
        .unwrap_or(default_port);
    let bind_address = std::net::SocketAddr::from(([127, 0, 0, 1], mcp_port));
    let cancellation_token = CancellationToken::new();

    tracing::info!("Starting MCP server on http://{}", bind_address);
    tokio::spawn(async move {
        if let Err(e) = run_http_server(
            engine,
            debug,
            builder_services,
            bind_address,
            cancellation_token,
        )
        .await
        {
            tracing::error!("MCP server error: {}", e);
        }
    });
}

/// Extension trait for registering MCP server services in a [`ServiceCollection`]
///
/// This trait provides a convenient method to register the MCP server
/// with a single call, taking just the port as a parameter.
///
/// # Example
///
/// ```rust,ignore
/// use holon_mcp::di::McpInjectorExt;
///
/// services.add_mcp_server(8000)?;
/// ```
pub trait McpInjectorExt {
    fn add_mcp_server(&self, port: u16) -> std::result::Result<(), fluxdi::Error>;
    fn add_mcp_server_with_config(
        &self,
        config: McpServerConfig,
    ) -> std::result::Result<(), fluxdi::Error>;
}

impl McpInjectorExt for Injector {
    fn add_mcp_server(&self, port: u16) -> std::result::Result<(), fluxdi::Error> {
        self.add_mcp_server_with_config(McpServerConfig::new(port))
    }

    fn add_mcp_server_with_config(
        &self,
        config: McpServerConfig,
    ) -> std::result::Result<(), fluxdi::Error> {
        self.provide::<McpServerConfig>(Provider::root(move |_| Shared::new(config.clone())));
        McpServerModule.configure(self)?;
        Ok(())
    }
}
