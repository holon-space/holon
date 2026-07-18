use anyhow::Result;
use gpui::*;
use holon_app::BootComponent;
use holon_app::BootError;
use holon_app::BootStage;
use holon_frontend::FrontendSession;
use holon_frontend::cli;
use holon_frontend::reactive::ReactiveEngine;
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
    // Production: don't block window paint on the OrgMode initial scan.
    // The FrontendSession factory spawns the wait + Loro seed in the
    // background; the reactive layer fills in data as it arrives. Tests
    // override this via `SessionConfig` to wait_for_ready=true.
    let session_config = session_config.without_wait();

    let runtime = tokio::runtime::Runtime::new()?;

    let boot_result = runtime.block_on(async {
        tracing::info!("Starting GPUI frontend...");

        let mut app = fluxdi::Application::new(GpuiModule {
            holon_config,
            session_config,
            config_dir,
            locked_keys: locked,
        });
        let timeout = std::time::Duration::from_secs(180);
        match tokio::time::timeout(timeout, app.bootstrap()).await {
            Err(_) => Err(BootError::new(
                BootComponent::Session,
                BootStage::SessionResolve,
                anyhow::anyhow!("Bootstrap timed out after {timeout:?}"),
            )),
            Ok(Err(e)) => Err(BootError::from_bootstrap_error(e)),
            Ok(Ok(())) => {
                tracing::info!("Session ready");
                Ok(app)
            }
        }
    });

    // Boot failed: emit a structured, component-attributed report (which
    // component, which stage, and the full source chain) and exit non-zero.
    // Increment 2 replaces this terminal exit with the recovery shell.
    let mut app = match boot_result {
        Ok(app) => app,
        Err(boot_err) => {
            eprint!("{}", boot_err.structured_report());
            std::process::exit(1);
        }
    };

    let injector = app.injector();
    let session = injector.resolve::<FrontendSession>();
    let engine = injector.resolve::<ReactiveEngine>();
    let debug = injector.resolve::<DebugServices>();

    // LoroSyncControllerHandle is resolved by the FrontendSession factory
    // in a background task that awaits OrgMode readiness first. We don't
    // block on it here — the window opens immediately and Loro seeding
    // happens concurrently with the first frames.

    let rt_handle = runtime.handle().clone();

    // Live oracles (debug builds): run the cheap tier of the keystone PBT
    // invariants as background checks against the live DB, so every manual
    // dogfood session carries oracles. HOLON_ORACLES=off opts out. The UI
    // bridge + banner are wired inside the window launch; the latency-SLO
    // layer is installed by `holon_frontend::logging::init` above.
    #[cfg(debug_assertions)]
    {
        let mode = holon_oracles::OracleMode::from_env();
        if mode.enabled() {
            let backend_engine = injector
                .try_resolve::<holon::api::backend_engine::BackendEngine>()
                .map_err(|e| anyhow::anyhow!("live oracles need BackendEngine from DI: {e}"))?;
            holon_gpui::oracles_ui::spawn_oracle_runner(backend_engine, &rt_handle, mode);
        }
    }

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
