use std::sync::Arc;

use anyhow::Result;
use gpui::*;
use holon_frontend::cli;
use holon_frontend::FrontendSession;

use holon_gpui::launch_holon_window;

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

    let config = cli::parse_args("holon-gpui")?;
    config.log_summary("GPUI");

    let runtime = tokio::runtime::Runtime::new()?;

    let (session, app_state) = runtime.block_on(async {
        let widgets = holon_gpui::render_supported_widgets();
        let frontend_config = cli::build_frontend_config(&config, widgets);
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
                    tracing::error!("MCP server error: {e}");
                }
            });
        }

        let root_id = holon_api::root_layout_block_uri();
        let watch = session.watch_ui(&root_id, true).await?;
        let app_state = holon_frontend::cdc::spawn_ui_listener(watch);

        tracing::info!("watch_ui({root_id}) stream established");
        Ok::<_, anyhow::Error>((session, app_state))
    })?;

    let rt_handle = runtime.handle().clone();
    let _runtime_guard = std::thread::spawn(move || {
        runtime.block_on(std::future::pending::<()>());
    });

    #[cfg(feature = "desktop")]
    {
        let app = Application::with_platform(gpui_platform::current_platform(false));
        app.run(move |cx| {
            launch_holon_window(session, app_state, rt_handle, cx);
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
