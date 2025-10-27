use anyhow::Result;
use rmcp::ServiceExt;
use std::fs::OpenOptions;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::{self, fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod telemetry;

use holon_mcp::server::{DebugServices, HolonMcpServer};

/// Create a default EnvFilter that suppresses noisy HTTP client and OpenTelemetry logs
fn default_env_filter() -> EnvFilter {
    // Some crates use dashes in target names, others use underscores - filter both variants
    EnvFilter::new(
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
         holon=debug",
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
    loro_enabled: bool,
}

fn parse_args() -> Result<Config> {
    let mut args = std::env::args().skip(1);
    let mut db_path = PathBuf::from(":memory:");
    let mut transport_mode = TransportMode::Stdio;
    let mut orgmode_root: Option<PathBuf> = None;
    let mut orgmode_loro_dir: Option<PathBuf> = None;
    let mut loro_enabled = std::env::var("HOLON_LORO_ENABLED")
        .map(|v| !v.is_empty() && v != "0" && v.to_lowercase() != "false")
        .unwrap_or(false);

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
                loro_enabled = true;
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
                eprintln!("  --orgmode-root PATH          OrgMode root directory (required for OrgMode features)");
                eprintln!("  --orgmode-loro-dir PATH      OrgMode Loro storage directory (default: {{orgmode-root}}/.loro)");
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
                eprintln!("  holon-mcp --orgmode-root /path/to/org         # Enable OrgMode with root directory");
                eprintln!("  holon-mcp --orgmode-root /org --orgmode-loro-dir /custom/loro  # Custom Loro storage");
                std::process::exit(0);
            }
            _ => {
                // Treat as database path if it doesn't start with --
                if !arg.starts_with("--") {
                    db_path = PathBuf::from(arg);
                }
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
) -> Result<()> {
    let server = HolonMcpServer::new(Some(engine), debug, None);
    use rmcp::transport::stdio;
    let running = server.serve(stdio()).await?;

    // Wait for the connection to close
    // This returns Result<QuitReason, JoinError>
    // QuitReason indicates why the server quit (e.g., connection closed, error, etc.)
    // Note: Connection closed errors are expected when stdin closes and should be handled gracefully
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
    holon_mcp::di::run_http_server(Some(engine), debug, None, bind_address, cancellation_token)
        .await
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse arguments first to determine transport mode
    let config = parse_args()?;

    // Configure logging based on transport mode
    match config.transport_mode {
        TransportMode::Stdio => {
            // In stdio mode, write all logs to a file to avoid interfering with protocol communication
            // Determine log file path
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
            let log_level =
                EnvFilter::try_from_default_env().unwrap_or_else(|_| default_env_filter());

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
            let log_level =
                EnvFilter::try_from_default_env().unwrap_or_else(|_| default_env_filter());

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

    // Relay mode: when HOLON_BROWSER_RELAY_URL is set, forward all tool calls to the
    // browser via the serve.mjs WebSocket hub. No local engine or DB needed.
    if std::env::var("HOLON_BROWSER_RELAY_URL").is_ok() {
        let relay_port: u16 = std::env::var("RELAY_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3002);
        let bind_address: std::net::SocketAddr = ([127, 0, 0, 1], relay_port).into();
        let cancellation_token = tokio_util::sync::CancellationToken::new();
        let token_for_signal = cancellation_token.clone();
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok(); // ALLOW(ok): signal failure is non-fatal
            token_for_signal.cancel();
        });
        let debug = Arc::new(DebugServices::default());
        holon_mcp::di::run_http_server(None, debug, None, bind_address, cancellation_token)
            .await?;
        return Ok(());
    }

    // Build HolonConfig from parsed MCP args
    let holon_config = holon_frontend::HolonConfig {
        db_path: Some(config.db_path),
        orgmode: holon_frontend::config::OrgmodeConfig {
            root_directory: config.orgmode_root,
        },
        loro: holon_frontend::config::LoroPreferences {
            enabled: if config.loro_enabled {
                Some(true)
            } else {
                None
            },
            storage_dir: config.orgmode_loro_dir,
        },
        ..Default::default()
    };
    let config_dir = holon_frontend::config::resolve_config_dir(None);
    let session_config =
        holon_frontend::SessionConfig::new(holon_api::UiInfo::permissive()).without_wait();

    let orgmode_root_for_debug = holon_config.orgmode.root_directory.clone();

    let app = {
        use fluxdi::{Injector, Module, ModuleLifecycleFuture, Shared};
        use holon_frontend::frontend_module::FrontendInjectorExt;

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

                holon::di::open_and_register_core(injector, db_path)
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
                        .try_resolve::<holon::sync::LoroBlockOperations>()
                        .ok()
                        .map(|ops| ops.shared_doc_store());
                    if let Some(store) = loro_doc_store {
                        debug.loro_doc_store.set(store).ok(); // ALLOW(ok): OnceLock already set
                    }
                    if let Some(root) = orgmode_root {
                        debug.orgmode_root.set(root).ok(); // ALLOW(ok): OnceLock already set
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
    let engine = injector
        .resolve::<holon_frontend::FrontendSession>()
        .engine()
        .clone();
    let debug = injector.resolve::<DebugServices>();

    // Run server based on transport mode
    match config.transport_mode {
        TransportMode::Stdio => {
            run_stdio_server(engine, debug).await?;
        }
        TransportMode::Http { bind_address } => {
            tracing::info!("Starting Holon MCP server in HTTP mode on {}", bind_address);
            run_http_server_standalone(engine, debug, bind_address).await?;
        }
    }

    Ok(())
}
