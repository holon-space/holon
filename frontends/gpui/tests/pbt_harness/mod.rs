//! Shared real-GPUI window harness for PBT-style test binaries.
//!
//! Both the random PBT (`gpui_ui_pbt`) and the Gherkin replay
//! (`gpui_gherkin_replay`) need the *same* GPUI plumbing: a main-thread
//! `Application` + window, a background thread that drives the SUT, and a
//! `GpuiUserDriver` injected after StartApp. They differ only in *what runs on
//! the background thread* — random generation vs fixture replay — which is a
//! `run` closure passed in here. This module owns everything else once.
//!
//! Included via `#[path = "pbt_harness/mod.rs"] mod pbt_harness;` from each
//! `harness = false` binary (subdir modules aren't auto-discovered as tests).

#![allow(dead_code)]

pub mod random_pbt;
pub mod random_pbt_sim;
pub mod sim_windowed_replay;
pub mod windowed_replay;
pub mod windowed_wide;

use std::sync::mpsc::sync_channel;
use std::sync::Arc;
use std::time::Duration;

use gpui::Application;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_integration_tests::pbt::phased::{PbtReadyContext, PbtReadyResult};
use holon_integration_tests::pbt::ui_harness::{
    screenshot_dir, set_loro_peer_id_if_unset, set_memory_multiplier_if_unset,
    spawn_quit_on_pbt_finish, try_start_embedded_mcp, wait_for_geometry_ready,
};
use holon_integration_tests::{GeometryDriver, XcapBackend};

/// The `on_ready` callback the SUT runner invokes once StartApp completes.
pub type OnReady = Box<dyn FnOnce(&PbtReadyContext) -> Option<PbtReadyResult> + Send>;

/// Extract the panic message from a caught payload (`panic!`/`format!` →
/// `String`; `expect`/`&str` literals → `&str`).
pub fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else {
        "<panic with non-string payload>".to_string()
    }
}

struct GpuiLaunchContext {
    engine: Arc<holon::api::BackendEngine>,
    session: Arc<holon_frontend::FrontendSession>,
    reactive_engine: Arc<holon_frontend::reactive::ReactiveEngine>,
    runtime_handle: tokio::runtime::Handle,
    debug_services: Arc<holon_mcp::server::DebugServices>,
}

/// Launch a real GPUI window and drive the SUT on a background thread via
/// `run`. `run` receives the screenshot `GeometryDriver` (used by the random
/// PBT, ignored by replay) and a ready-built `OnReady` that opens the window
/// and returns a real `GpuiUserDriver`; it must call the appropriate
/// `*_with_driver_sync_callback` runner with them and return its summary.
pub fn run_in_gpui_window<R>(window_title: &'static str, screenshot_label: &'static str, run: R)
where
    R: FnOnce(&mut GeometryDriver, OnReady) -> anyhow::Result<String> + Send + 'static,
{
    // Memory budgets are calibrated for the headless PBT; GPUI rendering adds
    // ~40MB RSS/transition, so relax them. Atomic editor primitives need a real
    // InputState, gated to GPUI runs by default.
    set_memory_multiplier_if_unset("15");
    set_loro_peer_id_if_unset("1");
    if std::env::var("PBT_ATOMIC_EDITOR").is_err() {
        std::env::set_var("PBT_ATOMIC_EDITOR", "1");
    }
    if std::env::var("PBT_MUTABLE_TEXT").is_err() {
        std::env::set_var("PBT_MUTABLE_TEXT", "1");
    }

    let (ctx_tx, ctx_rx) = sync_channel::<GpuiLaunchContext>(1);
    let (window_ready_tx, window_ready_rx) = sync_channel::<()>(1);

    let screenshot_dir = screenshot_dir(screenshot_label);
    let bounds_registry = BoundsRegistry::new();
    let visual_state: holon_integration_tests::ui_driver::VisualState =
        Arc::new(std::sync::Mutex::new(None));
    let debug = Arc::new(holon_mcp::server::DebugServices::default());

    let pbt_registry = bounds_registry.clone();
    let inv14_registry = bounds_registry.clone();
    let ready_registry = bounds_registry.clone();
    let driver_geometry: Arc<dyn GeometryProvider> = Arc::new(bounds_registry.clone());
    let pbt_visual_state = visual_state.clone();
    let inv14_visual_state = visual_state.clone();
    let pbt_debug = debug.clone();

    // Background thread: build the screenshot driver + signal watcher, the
    // window-launch `on_ready`, then hand both to the caller's runner.
    let pbt_handle = std::thread::spawn(move || {
        let backend = XcapBackend::new(window_title);
        let mut driver = GeometryDriver::new(Box::new(pbt_registry))
            .with_screenshots(Box::new(backend), screenshot_dir.clone())
            .with_visual_state(pbt_visual_state);
        let _signal_watcher = driver.spawn_signal_watcher();

        let on_ready: OnReady = Box::new(move |pbt_ctx: &PbtReadyContext| {
            ctx_tx
                .send(GpuiLaunchContext {
                    engine: pbt_ctx.engine.clone(),
                    session: pbt_ctx.session.clone(),
                    reactive_engine: pbt_ctx.reactive_engine.clone(),
                    runtime_handle: pbt_ctx.runtime_handle.clone(),
                    debug_services: pbt_ctx.debug_services.clone(),
                })
                .expect("failed to send launch context to main thread");

            eprintln!("[{window_title}] waiting for GPUI window to be ready...");
            window_ready_rx
                .recv_timeout(Duration::from_secs(120))
                .expect("timed out waiting for GPUI window to become ready");
            eprintln!("[{window_title}] GPUI window ready, continuing");

            let tx = pbt_debug
                .interaction_tx
                .get()
                .expect("interaction_tx not populated after window_ready")
                .clone();
            let gpui_driver: Arc<dyn holon_frontend::user_driver::UserDriver> =
                Arc::new(holon_gpui::user_driver::GpuiUserDriver::new(
                    tx,
                    driver_geometry.clone(),
                    pbt_ctx.reactive_engine.clone(),
                ));
            Some(PbtReadyResult {
                driver: Some(gpui_driver),
                frontend_engine: Some(pbt_ctx.reactive_engine.clone()),
                frontend_geometry: Some(Box::new(inv14_registry)),
                frontend_visual_state: Some(inv14_visual_state),
            })
        });

        match run(&mut driver, on_ready) {
            Ok(summary) => {
                eprintln!("[{window_title}] {summary}");
                eprintln!(
                    "[{window_title}] screenshots at {}",
                    screenshot_dir.display()
                );
            }
            Err(e) => {
                eprintln!("[{window_title}] FAILED: {e:?}");
                holon_integration_tests::test_tracing::flush_chrome_trace();
                std::process::exit(1);
            }
        }
    });

    // Main thread: launch the window sharing the PBT's ReactiveEngine.
    let launch_ctx = ctx_rx
        .recv_timeout(Duration::from_secs(60))
        .expect("timed out waiting for PbtReadyContext from background thread");

    try_start_embedded_mcp(
        &launch_ctx.runtime_handle,
        &launch_ctx.engine,
        &launch_ctx.reactive_engine,
        launch_ctx.debug_services.clone(),
        "PBT_MCP_PORT",
        window_title,
    );

    let (pbt_failed, quit_rx) =
        spawn_quit_on_pbt_finish(pbt_handle, "PBT_KEEP_WINDOW", window_title);
    let pbt_failed_for_app = pbt_failed.clone();

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
            window_title,
            cx,
        );

        let ready_registry = ready_registry.clone();
        std::thread::spawn(move || {
            let geometry: Arc<dyn GeometryProvider> = Arc::new(ready_registry);
            wait_for_geometry_ready(&geometry, Duration::from_secs(180), window_title);
            let _ = window_ready_tx.send(());
            eprintln!("[{window_title}] Window ready signal sent");
        });

        let pbt_failed_for_quit = pbt_failed_for_app.clone();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                // NOTE: no unconditional frame pump here — see the matching
                // comment in `windowed_replay::run_window` (pump frames break
                // per-keystroke pacing and drop typed characters).
                if quit_rx.try_recv().is_ok() {
                    holon_integration_tests::test_tracing::flush_chrome_trace();
                    if pbt_failed_for_quit.load(std::sync::atomic::Ordering::SeqCst) {
                        std::process::exit(1);
                    }
                    let _ = cx.update(|cx| cx.quit());
                    break;
                }
            }
            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });

    holon_integration_tests::test_tracing::flush_chrome_trace();
    if pbt_failed.load(std::sync::atomic::Ordering::SeqCst) {
        std::process::exit(1);
    }
}
