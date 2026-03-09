use anyhow::Result;
use gpui::*;
use holon_frontend::cli;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::FrontendSession;
use holon_gpui::di::GpuiModule;
use holon_gpui::launch_holon_window_with_engine_and_share;
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

    holon_frontend::shadow_builders::register_render_dsl_widget_names();

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

    // Force the Loro sync controller to start. Its provider factory also
    // runs `seed_loro_from_persistent_store`, which mirrors every row in
    // the `block` table into the global Loro doc — without this, blocks
    // created by `seed_default_layout` (which bypasses the
    // `OperationProvider`) never reach Loro and ops like `share_subtree`
    // fail with "block X not found in Loro tree".
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    match runtime.block_on(async {
        injector
            .try_resolve_async::<holon::sync::LoroSyncControllerHandle>()
            .await
    }) {
        Ok(_handle) => {
            tracing::info!(
                "[startup] LoroSyncControllerHandle resolved — Loro seeded + controller running"
            );
        }
        Err(e) => {
            tracing::error!(
                error = %e,
                "[startup] LoroSyncControllerHandle resolve failed — Loro will be out of sync \
                 with SQL; share/accept ops on seeded blocks will fail"
            );
        }
    }

    let rt_handle = runtime.handle().clone();

    // Shutdown flush: spawn a tokio task that awaits Ctrl+C and flushes
    // every in-flight shared-doc save before exit. The 150ms debounce
    // window in `SaveWorker` means pending edits could otherwise be
    // lost on SIGINT/Ctrl+C. `gpui_app.run()` below blocks the main
    // thread and never returns cleanly, so we `std::process::exit`
    // after flushing — this is the one place it's correct.
    #[cfg(all(
        feature = "desktop",
        not(all(target_arch = "wasm32", target_os = "unknown"))
    ))]
    {
        let injector_for_signal = injector.clone();
        rt_handle.spawn(async move {
            if let Err(e) = tokio::signal::ctrl_c().await {
                tracing::warn!("ctrl_c handler install failed: {e}");
                return;
            }
            tracing::info!("Ctrl+C received — flushing shared-tree snapshots");
            if let Ok(backend) = injector_for_signal
                .try_resolve::<std::sync::Arc<holon::sync::loro_share_backend::LoroShareBackend>>()
            {
                backend.flush_all().await;
                tracing::info!("flush_all complete");
            }
            std::process::exit(0);
        });
    }

    // Resolve the share backend up-front (feature-gated). The bridge is
    // wired inside `launch_holon_window_with_engine_and_share`. fluxdi
    // registers the backend as `Arc<LoroShareBackend>`, and `try_resolve`
    // wraps that in its own `Arc`, so we flatten with `(*arc).clone()`.
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    let share_backend: Option<
        std::sync::Arc<holon::sync::loro_share_backend::LoroShareBackend>,
    > = match injector
        .try_resolve::<std::sync::Arc<holon::sync::loro_share_backend::LoroShareBackend>>()
    {
        Ok(arc) => Some((*arc).clone()),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "[share-ui] resolving Arc<LoroShareBackend> from DI failed — \
                 share/accept ops will be inert"
            );
            None
        }
    };
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    let share_backend: Option<
        std::sync::Arc<holon::sync::loro_share_backend::LoroShareBackend>,
    > = None;

    #[cfg(feature = "desktop")]
    {
        let gpui_app = Application::with_platform(gpui_platform::current_platform(false));
        gpui_app.run(move |cx| {
            launch_holon_window_with_engine_and_share(
                session,
                engine,
                debug,
                share_backend,
                rt_handle,
                cx,
            );
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
