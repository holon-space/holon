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

    // PBT runs on background thread (GPUI needs main thread on macOS)
    let pbt_registry = bounds_registry.clone();
    let inv14_registry = bounds_registry.clone();
    let ready_registry = bounds_registry.clone();
    let pbt_visual_state = visual_state.clone();
    let inv14_visual_state = visual_state.clone();
    let pbt_focused_eid = focused_element_id.clone();
    let inv15_focused_eid = focused_element_id.clone();
    let pbt_handle = std::thread::spawn(move || {
        let backend = XcapBackend::new("Holon PBT");
        let mut driver = GeometryDriver::new(Box::new(pbt_registry))
            .with_screenshots(Box::new(backend), screenshot_dir.clone())
            .with_visual_state(pbt_visual_state)
            .with_focused_element_id(pbt_focused_eid);

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
                // F6: the channel-based GpuiUserDriver needs an
                // InteractionCommand channel that isn't plumbed through
                // PbtReadyContext yet. Leave `driver: None` until that
                // wiring exists — the phased loop falls back to
                // ReactiveEngineDriver automatically.
                Some(PbtReadyResult {
                    driver: None,
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

    let debug = std::sync::Arc::new(holon_mcp::server::DebugServices {
        focused_element_id: focused_element_id.clone(),
        ..Default::default()
    });

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

        // Wait for GPUI to render content widgets (not just block_ref wrappers).
        // Instead of a fixed delay, poll the BoundsRegistry until content
        // widgets appear. This adapts to debug vs release build speed.
        let ready_registry = ready_registry.clone();
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(90);
            loop {
                std::thread::sleep(Duration::from_millis(500));
                let elements = ready_registry.all_elements();
                let has_content = elements
                    .iter()
                    .any(|(_, info)| info.widget_type != "block_ref");
                if has_content {
                    eprintln!(
                        "[gpui_ui_pbt] Window ready: {} elements ({} content)",
                        elements.len(),
                        elements
                            .iter()
                            .filter(|(_, i)| i.widget_type != "block_ref")
                            .count(),
                    );
                    break;
                }
                if std::time::Instant::now() > deadline {
                    eprintln!(
                        "[gpui_ui_pbt] Window ready timeout — {} elements but no content widgets",
                        elements.len(),
                    );
                    break;
                }
            }
            let _ = window_ready_tx.send(());
            eprintln!("[gpui_ui_pbt] Window ready signal sent");
        });

        // When PBT thread finishes, quit the GPUI app.
        let (quit_tx, quit_rx) = std::sync::mpsc::sync_channel::<()>(1);
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(500));
            if pbt_handle.is_finished() {
                pbt_handle.join().expect("PBT thread panicked");
                let _ = quit_tx.send(());
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
}
