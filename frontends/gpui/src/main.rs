use anyhow::Result;
use gpui::*;
use holon_frontend::cli;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::FrontendSession;
use holon_gpui::di::GpuiModule;
use holon_gpui::launch_holon_window_with_engine;
use holon_mcp::server::DebugServices;

fn main() -> Result<()> {
    #[cfg(feature = "heap-profile")]
    let _heap_guard = holon_frontend::memory_monitor::heap_profile::start();

    let _log_guard = holon_frontend::logging::init();

    // Connect to the dx dev server so it can hot-patch via subsecond
    #[cfg(feature = "hot-reload")]
    {
        let ip = std::env::var("DIOXUS_DEVSERVER_IP").ok(); // ALLOW(ok): non-critical env var
        let port = std::env::var("DIOXUS_DEVSERVER_PORT").ok(); // ALLOW(ok): non-critical env var
        tracing::info!("hot-reload: DIOXUS_DEVSERVER_IP={ip:?}, DIOXUS_DEVSERVER_PORT={port:?}");
        dioxus_devtools::connect_subsecond();
    }

    let widgets = holon_gpui::render_supported_widgets();
    let (holon_config, session_config, config_dir, locked) = cli::build_session(widgets)?;

    let runtime = tokio::runtime::Runtime::new()?;

    let mut app = runtime.block_on(async {
        tracing::info!("Starting GPUI frontend...");

        let mut app = fluxdi::Application::new(GpuiModule {
            holon_config,
            session_config,
            config_dir,
            locked_keys: locked,
        });
        let timeout = std::time::Duration::from_secs(60);
        tokio::time::timeout(timeout, app.bootstrap())
            .await
            .map_err(|_| anyhow::anyhow!("Bootstrap timed out after {timeout:?}"))?
            .map_err(|e| anyhow::anyhow!("Bootstrap failed: {e}"))?;

        tracing::info!("Session ready");
        Ok::<_, anyhow::Error>(app)
    })?;

    let injector = app.injector();
    let session = injector.resolve::<FrontendSession>();
    let engine = injector.resolve::<ReactiveEngine>();
    let debug = injector.resolve::<DebugServices>();

    let rt_handle = runtime.handle().clone();

    #[cfg(feature = "desktop")]
    {
        let gpui_app = Application::with_platform(gpui_platform::current_platform(false));
        gpui_app.run(move |cx| {
            launch_holon_window_with_engine(session, engine, debug, rt_handle, cx);
            cx.activate(true);
        });
    }

    #[cfg(feature = "mobile")]
    {
        tracing::debug!("Mobile builds use android_main/ios_main, not this binary.");
    }

    // Graceful shutdown — fires GpuiModule::on_stop (MCP server stop, etc.)
    runtime.block_on(async {
        let timeout = std::time::Duration::from_secs(10);
        match tokio::time::timeout(timeout, app.shutdown()).await {
            Ok(Ok(())) => tracing::info!("Shutdown complete"),
            Ok(Err(e)) => tracing::warn!("Shutdown error: {e}"),
            Err(_) => tracing::warn!("Shutdown timed out after {timeout:?}"),
        }
    });

    Ok(())
}
