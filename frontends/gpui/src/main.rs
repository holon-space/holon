use std::sync::Arc;

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

    // Resolve the shared pending connector-write store (leases/read-write
    // ruling, increment 4c). `McpIntegrationsModule` registers it via
    // `provide::<PendingWriteStore>`, so `resolve` returns the shared
    // `Arc<PendingWriteStore>` directly. `None` when no MCP integrations exist
    // (no once_only writes are possible → no approve panel).
    // Degraded-disclosure bus. `add_frontend` registers it unconditionally, so
    // a missing one is a wiring bug, not a mode — resolve hard rather than ship
    // a window whose only degradation channel is the log.
    let degraded_bus: std::sync::Arc<holon::sync::DegradedSignalBus> =
        (*injector.resolve::<std::sync::Arc<holon::sync::DegradedSignalBus>>()).clone();

    let pending_writes: Option<std::sync::Arc<holon_app::PendingWriteStore>> =
        match injector.try_resolve::<holon_app::PendingWriteStore>() {
            Ok(store) => Some(store),
            Err(e) => {
                tracing::debug!(error = %e, "[pending-writes] no shared store in DI — approve panel inert");
                None
            }
        };

    // TEST MODE seam (single flag check): `HOLON_MCP_ALLOW_RESET` — the same
    // env that un-gates the MCP `reset_vault` tool — additionally routes the
    // desktop launch through the REBINDABLE window and installs the gpui-side
    // reset builder + reset pump (previously mobile-only, which made the
    // live-MCP keystone iOS-sim-only). Without the flag this arm is dead and
    // the desktop launch path is unchanged.
    let mcp_reset_test_mode = std::env::var("HOLON_MCP_ALLOW_RESET").is_ok();

    // Reset-safe debug handles for the MCP inspection tools (`render_org`,
    // `await_quiescence`, `debug_pbt_snapshot`); a later `reset_vault` swaps the
    // cell for the fresh session's handles.
    {
        let injector_for_cell = injector.clone();
        let session_for_cell = session.clone();
        let engine_for_cell = engine.clone();
        let cell = runtime.block_on(async move {
            let loro_sync_handle = injector_for_cell
                .try_resolve_async::<holon::sync::LoroSyncControllerHandle>()
                .await
                .ok();
            let block_query_source = Some(session_for_cell.block_query().clone());
            let org_idle_signal = injector_for_cell
                .try_resolve::<holon_orgmode::OrgSyncIdleSignal>()
                .ok();
            let loro_doc_store = injector_for_cell
                .try_resolve::<holon::sync::LoroBlockOperations>()
                .ok()
                .map(|ops| ops.shared_doc_store());
            let writeback_renderer = injector_for_cell
                .try_resolve_async::<holon_filesystem::WritebackRenderer>()
                .await
                .ok();
            holon_mcp::server::DebugHandlesCell {
                loro_sync_handle,
                org_idle_signal,
                block_query_source,
                loro_doc_store,
                reactive_engine: Some(engine_for_cell),
                writeback_renderer,
            }
        });
        *debug.live_debug.write().expect("live_debug cell poisoned") = cell;

        // The invariant catalog the suite runs lives in the pbt-only test crate,
        // so a release build carries no suite and `run_self_checks` reports that
        // absence as an error rather than an empty report.
        #[cfg(feature = "pbt")]
        {
            assert!(
                debug
                    .self_check_suite
                    .set(std::sync::Arc::new(
                        holon_integration_tests::pbt::live_self_check::LiveSelfCheck
                    ))
                    .is_ok(),
                "self_check_suite registered twice"
            );
        }
    }

    #[cfg(feature = "desktop")]
    {
        let gpui_app = Application::with_platform(gpui_platform::current_platform(false));
        gpui_app.run(move |cx| {
            // Install the pending-write store as a GPUI global so the window
            // wiring can spawn the bus bridge and the render pass can build the
            // approve panel (mirrors the DegradedToastSink/ShareTrigger globals).
            if let Some(store) = pending_writes {
                cx.set_global(holon_gpui::share_ui::PendingWritesGlobal(store));
            }
            if mcp_reset_test_mode {
                // Disclosed degraded/test mode — unmissable by design.
                tracing::warn!(
                    "MCP reset builder enabled — TEST MODE (HOLON_MCP_ALLOW_RESET): rebindable \
                     window, share/accept actions not wired (the degraded-disclosure bridge is)"
                );
                eprintln!("[holon] MCP reset builder enabled — TEST MODE (HOLON_MCP_ALLOW_RESET)");

                let mut nav = holon_gpui::navigation_state::NavigationState::with_input_router(
                    debug.input_router.clone(),
                );
                nav.set_navigation_debug(debug.navigation_state.clone());
                let bounds_registry = holon_gpui::geometry::BoundsRegistry::new();
                let handle = holon_gpui::launch_holon_window_rebindable(
                    session,
                    engine,
                    rt_handle,
                    nav,
                    bounds_registry,
                    Some(debug.clone()),
                    Some(degraded_bus),
                    "Holon",
                    cx,
                )
                .unwrap_or_else(|| {
                    eprintln!("[holon] rebindable Holon window failed to open");
                    std::process::exit(1);
                });

                // gpui-side reset builder: boots a fresh seeded SUT for the
                // (tokio) `reset_vault` tool. Mirrors the mobile install.
                let reset_builder: holon_mcp::server::ResetBuilderFn = Arc::new(|files| {
                    Box::pin(holon_gpui::reset::build_fresh_sut_from_files(files))
                        as futures::future::BoxFuture<
                            'static,
                            anyhow::Result<holon_mcp::server::ResetBuildOutput>,
                        >
                });
                debug.reset_builder.set(reset_builder).ok();

                // Main-thread reset pump: owns the `!Send` `RebindHandle` and
                // re-points the live window on each `ResetRequest`. This
                // repo's gpui fork makes `AsyncApp::update` infallible (it
                // returns the closure result directly), so the rebind always
                // runs on the main thread before the ack fires.
                let (reset_tx, mut reset_rx) =
                    futures::channel::mpsc::channel::<holon_mcp::server::ResetRequest>(4);
                debug.reset_tx.set(reset_tx).ok();
                cx.spawn(async move |cx| {
                    use futures::StreamExt;
                    while let Some(req) = reset_rx.next().await {
                        let holon_mcp::server::ResetRequest {
                            session,
                            engine,
                            ack,
                        } = req;
                        cx.update(|cx| handle.rebind(session, engine, cx));
                        ack.send(Ok(())).ok();
                    }
                })
                .detach();
            } else {
                launch_holon_window_with_engine_and_share(
                    session,
                    engine,
                    debug,
                    share_backend,
                    degraded_bus,
                    rt_handle,
                    cx,
                );
            }
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
