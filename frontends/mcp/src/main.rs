use std::fs::OpenOptions;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use rmcp::ServiceExt;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{self};

mod telemetry;

use holon_mcp::server::DebugServices;
use holon_mcp::server::HolonMcpServer;

/// The subscriber filter: `RUST_LOG` when set, else a default that suppresses
/// noisy HTTP client and OpenTelemetry logs. Built by the shared
/// `env_filter_with_default`, which also applies the `holon_latency` directive.
fn default_env_filter() -> EnvFilter {
    // Some crates use dashes in target names, others use underscores - filter both
    // variants
    holon_frontend::logging::env_filter_with_default(
        "info,reqwest=warn,hyper=warn,hyper_util=warn,h2=warn,tower=warn,opentelemetry=warn,\
         opentelemetry_sdk=warn,opentelemetry_http=warn,opentelemetry_otlp=warn,\
         opentelemetry-sdk=warn,opentelemetry-http=warn,opentelemetry-otlp=warn,holon=debug",
    )
}

#[derive(Debug, Clone)]
enum TransportMode {
    Stdio,
    Http { bind_address: SocketAddr },
}

struct Config {
    db_path: PathBuf,
    transport_mode: TransportMode,
    orgmode_root: Option<PathBuf>,
    orgmode_loro_dir: Option<PathBuf>,
    /// What this invocation said about the CRDT layer. `None` states no
    /// opinion, leaving the resolver's default; `Some(false)` is the explicit
    /// opt-out and must stay distinguishable from it.
    loro_enabled: Option<bool>,
}

/// Read `HOLON_CRDT_ENABLED` into the three states the resolver distinguishes.
fn parse_crdt_env(raw: Option<&str>) -> Option<bool> {
    raw.map(|v| !v.is_empty() && v != "0" && v.to_lowercase() != "false")
}

/// The `HolonConfig` this binary boots from its parsed arguments.
fn holon_config_for(config: Config) -> holon_frontend::HolonConfig {
    holon_frontend::HolonConfig {
        db_path: Some(config.db_path),
        vault: holon_frontend::config::VaultConfig {
            root: config.orgmode_root,
        },
        crdt: holon_frontend::config::CrdtPreferences {
            enabled: config.loro_enabled,
            storage_dir: config.orgmode_loro_dir,
        },
        ..Default::default()
    }
}

fn parse_args() -> Result<Config> {
    let mut args = std::env::args().skip(1);
    let mut db_path = PathBuf::from(":memory:");
    let mut db_path_set = false;
    let mut transport_mode = TransportMode::Stdio;
    let mut orgmode_root: Option<PathBuf> = None;
    let mut orgmode_loro_dir: Option<PathBuf> = None;
    let mut loro_enabled = parse_crdt_env(std::env::var("HOLON_CRDT_ENABLED").ok().as_deref());

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--http" | "-H" => {
                let addr_str = args.next().unwrap_or_else(|| "127.0.0.1:8000".to_string());
                let addr: SocketAddr = addr_str
                    .parse()
                    .map_err(|e| anyhow::anyhow!("Invalid bind address '{}': {}", addr_str, e))?;
                transport_mode = TransportMode::Http { bind_address: addr };
            }
            "--stdio" | "-S" => {
                transport_mode = TransportMode::Stdio;
            }
            "--orgmode-root" | "--orgmode-dir" => {
                let path_str = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--orgmode-root requires a path argument"))?;
                orgmode_root = Some(PathBuf::from(path_str));
            }
            "--orgmode-loro-dir" => {
                let path_str = args.next().ok_or_else(|| {
                    anyhow::anyhow!("--orgmode-loro-dir requires a path argument")
                })?;
                orgmode_loro_dir = Some(PathBuf::from(path_str));
            }
            "--loro" => {
                loro_enabled = Some(true);
            }
            "--help" | "-h" => {
                // Write help to stderr to avoid interfering with stdout in stdio mode
                eprintln!("Usage: holon-mcp [OPTIONS] [DATABASE_PATH]");
                eprintln!();
                eprintln!("Options:");
                eprintln!(
                    "  --http, -H [ADDRESS]         Run HTTP server (default: 127.0.0.1:8000)"
                );
                eprintln!("  --stdio, -S                  Run stdio server (default)");
                eprintln!(
                    "  --orgmode-root PATH          OrgMode root directory (required for OrgMode \
                     features)"
                );
                eprintln!(
                    "  --orgmode-loro-dir PATH      OrgMode Loro storage directory (default: \
                     {{orgmode-root}}/.loro)"
                );
                eprintln!("  --help, -h                   Show this help message");
                eprintln!();
                eprintln!("Examples:");
                eprintln!(
                    "  holon-mcp                                    # stdio mode with in-memory DB"
                );
                eprintln!(
                    "  holon-mcp /path/to/db.db                      # stdio mode with file DB"
                );
                eprintln!(
                    "  holon-mcp --http                              # HTTP mode on 127.0.0.1:8000"
                );
                eprintln!(
                    "  holon-mcp --orgmode-root /path/to/org         # Enable OrgMode with root \
                     directory"
                );
                eprintln!(
                    "  holon-mcp --orgmode-root /org --orgmode-loro-dir /custom/loro  # Custom \
                     Loro storage"
                );
                std::process::exit(0);
            }
            _ => {
                if arg.starts_with("--") {
                    return Err(anyhow::anyhow!("unknown option '{}'", arg));
                }
                if db_path_set {
                    return Err(anyhow::anyhow!(
                        "database path given twice: '{}' and '{}'",
                        db_path.display(),
                        arg
                    ));
                }
                db_path = PathBuf::from(arg);
                db_path_set = true;
            }
        }
    }

    Ok(Config {
        db_path,
        transport_mode,
        orgmode_root,
        orgmode_loro_dir,
        loro_enabled,
    })
}

async fn run_stdio_server(
    engine: std::sync::Arc<holon::api::backend_engine::BackendEngine>,
    debug: std::sync::Arc<DebugServices>,
    type_registry: Option<std::sync::Arc<holon_profiles::TypeRegistry>>,
) -> Result<()> {
    let server = HolonMcpServer::with_type_registry(Some(engine), type_registry, debug, None);
    use rmcp::transport::stdio;
    let running = server.serve(stdio()).await?;

    // Wait for the connection to close
    // This returns Result<QuitReason, JoinError>
    // QuitReason indicates why the server quit (e.g., connection closed, error,
    // etc.) Note: Connection closed errors are expected when stdin closes and
    // should be handled gracefully
    if let Err(join_err) = running.waiting().await {
        // The background task errored
        // Check if it's a panic
        if join_err.is_panic() {
            return Err(anyhow::anyhow!("MCP server task panicked"));
        }
        // For JoinError, check if it's a connection closed error
        // Connection closed is expected when stdin closes, so we should exit cleanly
        let error_msg = format!("{}", join_err).to_lowercase();
        if error_msg.contains("connection closed")
            || error_msg.contains("connectionclosed")
            || error_msg.contains("closed")
        {
            // This is expected - stdin was closed, server should exit cleanly
            // Don't treat this as an error
            return Ok(());
        }
        // For other errors, convert to anyhow::Error
        return Err(anyhow::anyhow!("MCP server error: {}", join_err));
    }
    // Server quit normally (Ok(QuitReason))
    Ok(())
}

async fn run_http_server_standalone(
    engine: std::sync::Arc<holon::api::backend_engine::BackendEngine>,
    debug: std::sync::Arc<DebugServices>,
    type_registry: Option<std::sync::Arc<holon_profiles::TypeRegistry>>,
    bind_address: SocketAddr,
) -> Result<()> {
    use tokio_util::sync::CancellationToken;

    // Create cancellation token that will be cancelled on Ctrl+C
    let cancellation_token = CancellationToken::new();
    let token_for_signal = cancellation_token.clone();

    // Spawn a task to handle Ctrl+C
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok(); // ALLOW(ok): signal failure is non-fatal
        tracing::info!("Received Ctrl+C, shutting down HTTP server...");
        token_for_signal.cancel();
    });

    tracing::info!("Holon MCP HTTP server starting on http://{}", bind_address);
    tracing::info!("MCP endpoint: http://{}/mcp", bind_address);

    // Use the shared run_http_server from di module
    holon_mcp::di::run_http_server(
        Some(engine),
        debug,
        None,
        type_registry,
        bind_address,
        cancellation_token,
    )
    .await
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse arguments first to determine transport mode
    let config = parse_args()?;

    // Configure logging based on transport mode
    match config.transport_mode {
        TransportMode::Stdio => {
            // In stdio mode, write all logs to a file to avoid interfering with protocol
            // communication Determine log file path
            let log_file_path = std::env::var("HOLON_MCP_LOG_FILE")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    // Default to temp directory with timestamp
                    let mut path = std::env::temp_dir();
                    // Use system time for timestamp
                    let timestamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    path.push(format!("holon-mcp-{}.log", timestamp));
                    path
                });

            // Create log file
            let log_file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_file_path)
                .map_err(|e| {
                    anyhow::anyhow!("Failed to create log file at {:?}: {}", log_file_path, e)
                })?;

            // Configure log level - use default filter if RUST_LOG not set
            let log_level = default_env_filter();

            // Build subscriber with all layers
            let registry = tracing_subscriber::registry();

            // Initialize OpenTelemetry providers if enabled
            let otel_enabled = std::env::var("OTEL_TRACES_EXPORTER").is_ok()
                || std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok();
            if otel_enabled {
                match telemetry::init_opentelemetry() {
                    Ok(()) => {
                        // Add OpenTelemetry layer
                        let telemetry_layer = telemetry::create_opentelemetry_layer();
                        registry
                            .with(telemetry_layer)
                            .with(log_level)
                            .with(
                                fmt::layer()
                                    .with_writer(log_file)
                                    .with_ansi(false)
                                    .with_target(true)
                                    .with_thread_ids(true)
                                    .with_file(true)
                                    .with_line_number(true),
                            )
                            .init();
                    }
                    Err(e) => {
                        eprintln!("Warning: Failed to initialize OpenTelemetry: {}", e);
                        eprintln!("Continuing without OpenTelemetry support");
                        registry
                            .with(log_level)
                            .with(
                                fmt::layer()
                                    .with_writer(log_file)
                                    .with_ansi(false)
                                    .with_target(true)
                                    .with_thread_ids(true)
                                    .with_file(true)
                                    .with_line_number(true),
                            )
                            .init();
                    }
                }
            } else {
                // Add EnvFilter and fmt layer (no OpenTelemetry)
                registry
                    .with(log_level)
                    .with(
                        fmt::layer()
                            .with_writer(log_file)
                            .with_ansi(false)
                            .with_target(true)
                            .with_thread_ids(true)
                            .with_file(true)
                            .with_line_number(true),
                    )
                    .init();
            }

            // Write log file location to stderr once (before protocol starts)
            eprintln!("Holon MCP server started in stdio mode");
            eprintln!("Logs are being written to: {}", log_file_path.display());
            eprintln!("Set HOLON_MCP_LOG_FILE to specify a custom log file location");
        }
        TransportMode::Http { .. } => {
            // In HTTP mode, normal stderr logging is fine
            let log_level = default_env_filter();

            // Build subscriber with all layers
            let registry = tracing_subscriber::registry();

            // Initialize OpenTelemetry providers if enabled
            let otel_enabled = std::env::var("OTEL_TRACES_EXPORTER").is_ok()
                || std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok();
            if otel_enabled {
                match telemetry::init_opentelemetry() {
                    Ok(()) => {
                        // Add OpenTelemetry layer
                        let telemetry_layer = telemetry::create_opentelemetry_layer();
                        registry
                            .with(telemetry_layer)
                            .with(log_level)
                            .with(fmt::layer().with_writer(std::io::stderr).with_ansi(false))
                            .init();
                    }
                    Err(e) => {
                        eprintln!("Warning: Failed to initialize OpenTelemetry: {}", e);
                        eprintln!("Continuing without OpenTelemetry support");
                        registry
                            .with(log_level)
                            .with(fmt::layer().with_writer(std::io::stderr).with_ansi(false))
                            .init();
                    }
                }
            } else {
                // Add EnvFilter and fmt layer (no OpenTelemetry)
                registry
                    .with(log_level)
                    .with(fmt::layer().with_writer(std::io::stderr).with_ansi(false))
                    .init();
            }
        }
    }

    // Relay mode: when HOLON_BROWSER_RELAY_URL is set, forward all tool calls to
    // the browser via the serve.mjs WebSocket hub. No local engine or DB
    // needed.
    if std::env::var("HOLON_BROWSER_RELAY_URL").is_ok() {
        let relay_port: u16 = std::env::var("RELAY_PORT")
            .ok()
            .and_then(|s| s.parse().ok()) // ALLOW(ok): non-critical env var parse
            .unwrap_or(3002);
        let bind_address: std::net::SocketAddr = ([127, 0, 0, 1], relay_port).into();
        let cancellation_token = tokio_util::sync::CancellationToken::new();
        let token_for_signal = cancellation_token.clone();
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok(); // ALLOW(ok): signal failure is non-fatal
            token_for_signal.cancel();
        });
        let debug = Arc::new(DebugServices::default());
        holon_mcp::di::run_http_server(None, debug, None, None, bind_address, cancellation_token)
            .await?;
        return Ok(());
    }

    let transport_mode = config.transport_mode.clone();
    let holon_config = holon_config_for(config);
    let config_dir = holon_frontend::config::resolve_config_dir(None);
    let session_config = holon_frontend::SessionConfig::new(holon_api::UiInfo::permissive());

    let orgmode_root_for_debug = holon_config.vault.root.clone();

    let app = {
        use fluxdi::Injector;
        use fluxdi::Module;
        use fluxdi::ModuleLifecycleFuture;
        use fluxdi::Shared;
        use holon_app::FrontendInjectorExt;

        fn to_di_err(phase: &str, e: &dyn std::fmt::Display) -> fluxdi::Error {
            fluxdi::Error::module_lifecycle_failed("McpStandaloneModule", phase, &e.to_string())
        }

        struct McpStandaloneModule {
            holon_config: holon_frontend::HolonConfig,
            session_config: holon_frontend::SessionConfig,
            config_dir: std::path::PathBuf,
            orgmode_root: Option<std::path::PathBuf>,
        }

        impl Module for McpStandaloneModule {
            fn configure(&self, injector: &Injector) -> std::result::Result<(), fluxdi::Error> {
                let db_path = self.holon_config.resolve_db_path(&self.config_dir);

                holon::di::open_and_register_core(
                    injector,
                    db_path,
                    holon::di::StorageSelector::Turso,
                )
                .map_err(|e| to_di_err("configure", &e))?;

                injector
                    .add_frontend(
                        self.holon_config.clone(),
                        self.session_config.clone(),
                        self.config_dir.clone(),
                        std::collections::HashSet::new(),
                    )
                    .map_err(|e| to_di_err("configure", &e))?;

                holon_mcp::di::register_debug_services(injector);

                Ok(())
            }

            fn on_start(&self, injector: Shared<Injector>) -> ModuleLifecycleFuture {
                let orgmode_root = self.orgmode_root.clone();
                Box::pin(async move {
                    let _session = injector
                        .resolve_async::<holon_frontend::FrontendSession>()
                        .await;

                    // Populate DebugServices with Loro doc store + orgmode root
                    let debug = injector.resolve::<DebugServices>();
                    // ALLOW(ok): optional DI service
                    let loro_doc_store = injector
                        .try_resolve::<holon_loro::LoroBlockOperations>()
                        .ok()
                        .map(|ops| ops.shared_doc_store());
                    if let Some(store) = loro_doc_store {
                        debug.loro_doc_store.set(store).ok(); // ALLOW(ok):
                        // OnceLock already
                        // set
                    }
                    if let Some(root) = orgmode_root {
                        debug.orgmode_root.set(root).ok(); // ALLOW(ok):
                        // OnceLock already
                        // set
                    }

                    Ok(())
                })
            }
        }

        let mut app = fluxdi::Application::new(McpStandaloneModule {
            holon_config,
            session_config,
            config_dir,
            orgmode_root: orgmode_root_for_debug,
        });
        app.bootstrap()
            .await
            .map_err(|e| anyhow::anyhow!("Bootstrap failed: {e}"))?;
        app
    };

    let injector = app.injector();
    // Resolve the BackendEngine directly — `FrontendSession` no longer exposes
    // `engine()` (ADR 0004 Phase 9). The MCP binary is a Turso wiring, so the
    // engine is registered in the container.
    let engine = injector.resolve::<holon::api::backend_engine::BackendEngine>();
    let debug = injector.resolve::<DebugServices>();
    // The live entity registry the link classifier reads. Without it every
    // `[[<entity>:<id>]]` an agent writes through `dense_patch` degrades to an
    // unknown-scheme link and loses its `block_links` row.
    let type_registry = Some(injector.resolve::<holon_profiles::TypeRegistry>());

    // Shutdown flush: spawn a task that awaits Ctrl+C and flushes any
    // in-flight shared-doc saves before the process exits. The 150ms
    // debounce window in `SaveWorker` would otherwise drop pending
    // edits on SIGINT. HTTP mode already has its own ctrl_c handler
    // for the cancellation token — this one targets the flush side
    // only and does not exit (the server future completes naturally
    // once cancellation propagates).
    //
    // Relies on `holon` being compiled with its default `iroh-sync`
    // feature; `try_resolve` returns Err if the backend isn't
    // registered (e.g. iroh-sync disabled) so this is a no-op then.
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    {
        let injector_for_signal = injector.clone();
        tokio::spawn(async move {
            if let Err(e) = tokio::signal::ctrl_c().await {
                tracing::warn!("ctrl_c handler install failed: {e}");
                return;
            }
            tracing::info!("Ctrl+C received — flushing shared-tree snapshots");
            if let Ok(backend) = injector_for_signal
                .try_resolve::<std::sync::Arc<holon_loro::loro_share_backend::LoroShareBackend>>()
            {
                backend.flush_all().await;
                tracing::info!("flush_all complete");
            }
        });
    }

    // Run server based on transport mode
    match transport_mode {
        TransportMode::Stdio => {
            run_stdio_server(engine, debug, type_registry).await?;
        }
        TransportMode::Http { bind_address } => {
            tracing::info!("Starting Holon MCP server in HTTP mode on {}", bind_address);
            run_http_server_standalone(engine, debug, type_registry, bind_address).await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `HOLON_CRDT_ENABLED=false` is the documented opt-out
    /// (`holon-frontend/src/config.rs::crdt_enabled`). It must survive the trip
    /// through this binary's own env parse and reach `HolonConfig` as an
    /// explicit `false` — collapsing it into "absent" hands the resolver the
    /// CRDT default instead, and the user who asked for SqlOnly boots Loro.
    #[test]
    fn the_env_opt_out_reaches_the_resolved_config() {
        let cases = [
            (Some("false"), false),
            (Some("FALSE"), false),
            (Some("0"), false),
            (Some(""), false),
            (Some("true"), true),
            (Some("1"), true),
        ];
        for (raw, expected) in cases {
            let parsed = parse_crdt_env(raw);
            assert_eq!(
                parsed,
                Some(expected),
                "HOLON_CRDT_ENABLED={raw:?} must parse to an explicit Some({expected})"
            );
            let config = holon_config_for(sample_config(parsed));
            assert_eq!(
                config.crdt_enabled(),
                expected,
                "HOLON_CRDT_ENABLED={raw:?} must resolve to crdt_enabled() == {expected}"
            );
        }
    }

    /// No env var and no `--loro` flag: the binary states no opinion, so the
    /// resolver's default applies.
    #[test]
    fn an_unset_env_leaves_the_default_to_the_resolver() {
        assert_eq!(parse_crdt_env(None), None);
        let config = holon_config_for(sample_config(None));
        assert_eq!(config.crdt.enabled, None);
        assert!(config.crdt_enabled());
    }

    fn sample_config(loro_enabled: Option<bool>) -> Config {
        Config {
            db_path: PathBuf::from(":memory:"),
            transport_mode: TransportMode::Stdio,
            orgmode_root: None,
            orgmode_loro_dir: None,
            loro_enabled,
        }
    }
}
