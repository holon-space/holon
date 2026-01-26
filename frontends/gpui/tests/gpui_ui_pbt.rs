//! GPUI UI PBT — geometry-based PBT test with a real GPUI window and xcap screenshots.
//!
//! Launches a real GPUI window sharing the same Turso DB as the PBT engine.
//! BoundsRegistry populates during GPUI render passes, enabling the GeometryDriver
//! to look up element positions and xcap to capture window screenshots.
//!
//! Thread model:
//! - Main thread: GPUI event loop (required on macOS — `Application` must be on main thread)
//! - Background thread: PBT state machine + driver
//!
//! Synchronization:
//! 1. PBT runs pre-startup steps → on_ready sends context → blocks waiting for window
//! 2. Main thread receives context → launches GPUI window with shared ReactiveEngine
//! 3. PBT thread unblocks → runs post-startup steps with real window + xcap screenshots
//!
//! Key: the GPUI window reuses the PBT's ReactiveEngine (from DI), so all watch_ui
//! tasks, CDC subscriptions, and signal wakers run on the same tokio runtime. This
//! avoids cross-executor waker issues that would cause a blank window.
//!
//! This is a `harness = false` test binary (GPUI requires main thread).
//! Run with: cargo test -p holon-gpui --test gpui_ui_pbt

use std::sync::mpsc::sync_channel;
use std::sync::Arc;
use std::time::Duration;

use gpui::Application;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_integration_tests::pbt::phased::{
    run_pbt_with_driver_sync_callback, PbtReadyContext, PbtReadyResult,
};
use holon_integration_tests::{GeometryDriver, XcapBackend};

/// Context sent from the PBT thread to the main thread for GPUI window creation.
struct GpuiLaunchContext {
    session: Arc<holon_frontend::FrontendSession>,
    reactive_engine: Arc<holon_frontend::reactive::ReactiveEngine>,
    runtime_handle: tokio::runtime::Handle,
}

fn main() {
    // The PBT framework's `SpanCollector::global()` initializes the
    // process-wide tracing subscriber on first use (see
    // `crates/holon-integration-tests/src/test_tracing.rs`). When the
    // `chrome-trace` feature is enabled it also wires in a
    // `tracing_chrome` layer that writes a Chrome Trace Event JSON file
    // (default `./trace-{ts}.json`, override with
    // `CHROME_TRACE_FILE=/path`). View the result in
    // https://ui.perfetto.dev/. The guard is kept alive in the
    // `OnceLock` so it flushes on process exit.

    // Memory budgets are calibrated for the headless PBT. With GPUI running,
    // concurrent rendering adds ~40MB RSS per transition. Relax the memory
    // limits to avoid false positives.
    if std::env::var("PBT_MEMORY_MULTIPLIER").is_err() {
        // Navigation re-renders allocate ~100MB in GPUI entity cache and
        // render trees; use 15x to stay well above the headless limits.
        std::env::set_var("PBT_MEMORY_MULTIPLIER", "15");
    }

    // Channels for cross-thread coordination
    let (ctx_tx, ctx_rx) = sync_channel::<GpuiLaunchContext>(1);
    let (window_ready_tx, window_ready_rx) = sync_channel::<()>(1);

    let screenshot_dir = std::env::current_dir()
        .unwrap()
        .join("target")
        .join("pbt-screenshots")
        .join("gpui");

    let bounds_registry = BoundsRegistry::new();
    let visual_state: holon_integration_tests::ui_driver::VisualState =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let focused_element_id: holon_integration_tests::FocusedElementId =
        std::sync::Arc::new(std::sync::RwLock::new(None));

    // Shared DebugServices — the GPUI window's `setup_interaction_pump` populates
    // `debug.interaction_tx` (and `debug.user_driver`) once the window is up.
    // We share the Arc with the PBT thread so it can read the channel after
    // `window_ready_rx` fires and build a real `GpuiUserDriver` that routes
    // chord input through GPUI's `PlatformInput` pipeline (Tab → IndentInline
    // → operation dispatch) instead of the headless `ReactiveEngineDriver`.
    let debug = std::sync::Arc::new(holon_mcp::server::DebugServices {
        focused_element_id: focused_element_id.clone(),
        ..Default::default()
    });

    // PBT runs on background thread (GPUI needs main thread on macOS)
    let pbt_registry = bounds_registry.clone();
    let inv14_registry = bounds_registry.clone();
    let ready_registry = bounds_registry.clone();
    let driver_geometry: Arc<dyn holon_frontend::geometry::GeometryProvider> =
        Arc::new(bounds_registry.clone());
    let pbt_visual_state = visual_state.clone();
    let inv14_visual_state = visual_state.clone();
    let pbt_focused_eid = focused_element_id.clone();
    let inv15_focused_eid = focused_element_id.clone();
    let pbt_debug = debug.clone();
    let pbt_handle = std::thread::spawn(move || {
        let backend = XcapBackend::new("Holon PBT");
        let mut driver = GeometryDriver::new(Box::new(pbt_registry))
            .with_screenshots(Box::new(backend), screenshot_dir.clone())
            .with_visual_state(pbt_visual_state)
            .with_focused_element_id(pbt_focused_eid);

        let _signal_watcher = driver.spawn_signal_watcher();

        let result =
            run_pbt_with_driver_sync_callback(50, &mut driver, |pbt_ctx: &PbtReadyContext| {
                // Send the DI-resolved context to main thread for GPUI window creation.
                // This ensures the GPUI window shares the same ReactiveEngine, session,
                // and tokio runtime as the PBT — no cross-executor waker issues.
                ctx_tx
                    .send(GpuiLaunchContext {
                        session: pbt_ctx.session.clone(),
                        reactive_engine: pbt_ctx.reactive_engine.clone(),
                        runtime_handle: pbt_ctx.runtime_handle.clone(),
                    })
                    .expect("failed to send context to main thread");

                // Block until GPUI window is open and has rendered at least once
                eprintln!("[gpui_ui_pbt] PBT waiting for GPUI window to be ready...");
                window_ready_rx
                    .recv_timeout(Duration::from_secs(120))
                    .expect("timed out waiting for GPUI window to become ready");
                eprintln!("[gpui_ui_pbt] GPUI window ready, continuing PBT steps");

                // Build a real `GpuiUserDriver` that routes through the GPUI
                // window's input pump: `send_key_chord` → mouse-click-to-focus
                // → keystroke → keymap → action → operation dispatch. This
                // replaces the headless `ReactiveEngineDriver` fallback so
                // chord-driven block-tree ops (Tab/Shift+Tab/Cmd+ArrowUp/...)
                // exercise the same path real users hit.
                let tx = pbt_debug
                    .interaction_tx
                    .get()
                    .expect(
                        "interaction_tx not populated after window_ready — \
                         setup_interaction_pump should have run by now",
                    )
                    .clone();
                let gpui_driver = holon_gpui::user_driver::GpuiUserDriver::new(
                    tx,
                    driver_geometry.clone(),
                    pbt_ctx.reactive_engine.clone(),
                );
                Some(PbtReadyResult {
                    driver: Some(Box::new(gpui_driver)),
                    frontend_engine: Some(pbt_ctx.reactive_engine.clone()),
                    frontend_geometry: Some(Box::new(inv14_registry)),
                    frontend_visual_state: Some(inv14_visual_state),
                    frontend_focused_element_id: Some(inv15_focused_eid),
                })
            });

        match result {
            Ok(summary) => {
                eprintln!("[gpui_ui_pbt] {summary}");
                eprintln!("[gpui_ui_pbt] screenshots at {}", screenshot_dir.display());
            }
            Err(e) => {
                eprintln!("GPUI UI PBT failed: {e:?}");
                // Flush the chrome trace before bypassing static dtors
                // — `OnceLock`-stored guards don't run at `process::exit`,
                // so the JSON would be left truncated (no closing `]`).
                holon_integration_tests::test_tracing::flush_chrome_trace();
                std::process::exit(1);
            }
        }
    });

    // Main thread: wait for context from PBT, then launch GPUI
    let launch_ctx = ctx_rx
        .recv_timeout(Duration::from_secs(60))
        .expect("timed out waiting for PbtReadyContext from PBT thread");

    // Optional MCP server for live inspection (set PBT_MCP_PORT=8521 to enable).
    // Connect holon-direct to this port to inspect DB state while the test runs.
    if let Ok(port_str) = std::env::var("PBT_MCP_PORT") {
        let port: u16 = port_str.parse().expect("PBT_MCP_PORT must be a u16");
        let engine = Some(launch_ctx.session.engine().clone());
        let services: Arc<dyn holon_frontend::reactive::BuilderServices> =
            launch_ctx.reactive_engine.clone();
        let _guard = launch_ctx.runtime_handle.enter();
        holon_mcp::di::start_embedded_mcp_server(engine, Some(services), port);
        eprintln!("[gpui_ui_pbt] MCP server starting on port {port}");
        // Give the server time to bind before GPUI takes the main thread
        std::thread::sleep(Duration::from_secs(2));
        eprintln!("[gpui_ui_pbt] MCP server should be ready on port {port}");
    }

    let pbt_failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let pbt_failed_for_watcher = pbt_failed.clone();

    let app = Application::with_platform(gpui_platform::current_platform(false));

    app.run(move |cx| {
        cx.activate(true);

        let nav = holon_gpui::navigation_state::NavigationState::new();
        holon_gpui::launch_holon_window_with_title(
            launch_ctx.session,
            launch_ctx.reactive_engine,
            launch_ctx.runtime_handle,
            nav,
            bounds_registry,
            Some(debug),
            "Holon PBT",
            cx,
        );

        // Wait for GPUI to render real content widgets. The previous gate
        // (`widget_type != "live_block"`) fired on auto-`tag()` placeholders
        // (loading/row/spacer) while every panel was still showing the
        // `loading()` builder. Tighten to require an element that came from
        // a `tracked()` call (`render_entity`/`editable_text`/`selectable`),
        // signalled by `has_content && entity_id.is_some()`.
        //
        // Note: the gate fires on the first such element — typically a
        // sidebar selectable. The Main panel is intrinsically empty until
        // after a sidebar click populates `focus_roots`, so it would be a
        // bug to wait for Main content before starting the test.
        let ready_registry = ready_registry.clone();
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(180);
            loop {
                std::thread::sleep(Duration::from_millis(500));
                let elements = ready_registry.all_elements();
                let has_real_content = elements
                    .iter()
                    .any(|(_, info)| info.has_content && info.entity_id.is_some());
                if has_real_content {
                    let n_content = elements.iter().filter(|(_, i)| i.has_content).count();
                    eprintln!(
                        "[gpui_ui_pbt] Window ready: {} elements ({} with has_content)",
                        elements.len(),
                        n_content,
                    );
                    break;
                }
                if std::time::Instant::now() > deadline {
                    use std::collections::BTreeMap;
                    let mut hist: BTreeMap<String, usize> = BTreeMap::new();
                    for (_, info) in &elements {
                        *hist.entry(info.widget_type.clone()).or_default() += 1;
                    }
                    let sample_ids: Vec<String> = elements
                        .iter()
                        .filter_map(|(_, i)| i.entity_id.clone())
                        .take(5)
                        .collect();
                    eprintln!(
                        "[gpui_ui_pbt] Window ready timeout — {} elements, widget_type hist={:?}, \
                         has_content count={}, sample entity_ids={:?}",
                        elements.len(),
                        hist,
                        elements.iter().filter(|(_, i)| i.has_content).count(),
                        sample_ids,
                    );
                    break;
                }
            }
            let _ = window_ready_tx.send(());
            eprintln!("[gpui_ui_pbt] Window ready signal sent");
        });

        // When PBT thread finishes, quit the GPUI app (unless PBT_KEEP_WINDOW=1).
        let keep_window = std::env::var("PBT_KEEP_WINDOW").is_ok();
        let pbt_failed = pbt_failed_for_watcher;
        let (quit_tx, quit_rx) = std::sync::mpsc::sync_channel::<()>(1);
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(500));
            if pbt_handle.is_finished() {
                let thread_result = pbt_handle.join();
                if thread_result.is_err() {
                    pbt_failed.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                if keep_window {
                    eprintln!("[gpui_ui_pbt] PBT finished, keeping window open (PBT_KEEP_WINDOW)");
                } else {
                    if let Err(e) = &thread_result {
                        eprintln!("[gpui_ui_pbt] PBT thread panicked: {e:?}");
                    }
                    let _ = quit_tx.send(());
                }
                break;
            }
        });
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                if quit_rx.try_recv().is_ok() {
                    let _ = cx.update(|cx| cx.quit());
                    break;
                }
            }
            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });

    eprintln!("[gpui_ui_pbt] app.run returned, flushing chrome trace");
    // Always flush — static dtors don't run for `OnceLock`-stored
    // guards on either `process::exit` or normal return; this is the
    // only path that produces a complete chrome trace JSON.
    holon_integration_tests::test_tracing::flush_chrome_trace();

    if pbt_failed.load(std::sync::atomic::Ordering::SeqCst) {
        eprintln!("[gpui_ui_pbt] exiting with PBT failure");
        std::process::exit(1);
    }
    eprintln!("[gpui_ui_pbt] exiting cleanly");
}
