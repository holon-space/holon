use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use gpui::*;
use holon_frontend::{FrontendConfig, FrontendSession};

use holon_gpui::{launch_holon_window, state::AppState};

struct CliConfig {
    db_path: Option<PathBuf>,
    orgmode_root: Option<PathBuf>,
    loro_enabled: bool,
}

fn parse_args() -> Result<CliConfig> {
    let mut args = std::env::args().skip(1);
    let mut db_path: Option<PathBuf> = std::env::var("HOLON_DB_PATH").ok().map(PathBuf::from);
    let mut orgmode_root: Option<PathBuf> =
        std::env::var("HOLON_ORGMODE_ROOT").ok().map(PathBuf::from);
    let mut loro_enabled = std::env::var("HOLON_LORO_ENABLED")
        .map(|v| !v.is_empty() && v != "0" && v.to_lowercase() != "false")
        .unwrap_or(false);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--orgmode-root" | "--orgmode-dir" => {
                let path_str = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--orgmode-root requires a path argument"))?;
                orgmode_root = Some(PathBuf::from(path_str));
            }
            "--loro" => {
                loro_enabled = true;
            }
            "--help" | "-h" => {
                eprintln!("Usage: holon-gpui [OPTIONS] [DATABASE_PATH]");
                eprintln!();
                eprintln!("Options:");
                eprintln!("  --orgmode-root PATH  OrgMode root directory");
                eprintln!("  --loro               Enable Loro CRDT layer");
                eprintln!("  --help, -h           Show this help message");
                std::process::exit(0);
            }
            _ => {
                if !arg.starts_with("--") {
                    db_path = Some(PathBuf::from(arg));
                }
            }
        }
    }

    Ok(CliConfig {
        db_path,
        orgmode_root,
        loro_enabled,
    })
}

fn build_frontend_config(cli: &CliConfig) -> FrontendConfig {
    let widgets = holon_gpui::render_supported_widgets();
    let ui_info = holon_api::UiInfo {
        available_widgets: widgets,
        screen_size: None,
    };
    let mut config = FrontendConfig::new(ui_info);

    if let Some(ref db) = cli.db_path {
        config = config.with_db_path(db.clone());
    }
    if let Some(ref org) = cli.orgmode_root {
        config = config.with_orgmode(org.clone());
    }
    if cli.loro_enabled {
        config = config.with_loro();
    }

    config
}

fn main() -> Result<()> {
    #[cfg(feature = "chrome-trace")]
    let (_chrome_trace_guard, _chrome_trace_layer_set) = {
        use tracing_subscriber::layer::SubscriberExt;
        let (chrome_layer, guard) = holon_frontend::memory_monitor::chrome_trace::layer();
        let subscriber = tracing_subscriber::Registry::default()
            .with(chrome_layer)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .with_ansi(true),
            )
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "holon_gpui=info,holon=info".into()),
            );
        tracing::subscriber::set_global_default(subscriber)
            .expect("Failed to set tracing subscriber");
        (guard, true)
    };

    #[cfg(not(feature = "chrome-trace"))]
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "holon_gpui=info,holon=info".into()),
        )
        .init();

    let config = parse_args()?;

    eprintln!(
        "GPUI frontend: db={}, orgmode={:?}, loro={}",
        config
            .db_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or("(temp)".into()),
        config.orgmode_root,
        config.loro_enabled
    );

    let runtime = tokio::runtime::Runtime::new()?;

    let (session, app_state, watch_handle) = runtime.block_on(async {
        let frontend_config = build_frontend_config(&config);
        tracing::info!("Starting GPUI frontend...");
        let session = Arc::new(FrontendSession::new(frontend_config).await?);

        // Start MCP server
        {
            let mcp_engine = session.engine().clone();
            let mcp_port: u16 = std::env::var("MCP_SERVER_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(8520);
            let bind_address = std::net::SocketAddr::from(([127, 0, 0, 1], mcp_port));
            let cancellation_token = tokio_util::sync::CancellationToken::new();
            tracing::info!("Starting MCP server on http://{}", bind_address);
            tokio::spawn(async move {
                if let Err(e) = holon_mcp::di::run_http_server(
                    mcp_engine,
                    Arc::new(holon_mcp::server::DebugServices::default()),
                    bind_address,
                    cancellation_token,
                )
                .await
                {
                    tracing::error!("MCP server error: {}", e);
                }
            });
        }

        let root_id = holon_api::ROOT_LAYOUT_BLOCK_ID.to_string();
        let app_state = AppState::new(holon_api::widget_spec::WidgetSpec::from_rows(vec![]));

        let watch = session.watch_ui(root_id.clone(), None, true).await?;

        tracing::info!("watch_ui({root_id}) stream established");
        Ok::<_, anyhow::Error>((session, app_state, watch))
    })?;

    let rt_handle = runtime.handle().clone();
    let _runtime_guard = std::thread::spawn(move || {
        runtime.block_on(std::future::pending::<()>());
    });

    #[cfg(feature = "desktop")]
    {
        let app = Application::with_platform(gpui_platform::current_platform(false));
        app.run(move |cx| {
            launch_holon_window(session, app_state, watch_handle, rt_handle, cx);
        });
    }

    #[cfg(feature = "mobile")]
    {
        // On mobile, the app is launched via mobile.rs entry points
        // (android_main / ios_main). This binary is not used.
        eprintln!("Mobile builds use android_main/ios_main, not this binary.");
    }

    Ok(())
}
