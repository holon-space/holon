//! TUI UI PBT — geometry-based PBT against the same shared state machine
//! `frontends/gpui/tests/gpui_ui_pbt.rs` exercises against GPUI.
//!
//! Mirrors `gpui_ui_pbt` topologically: a PBT thread runs the property
//! state machine on a background thread; the main thread owns the
//! renderer; a shared `Arc<DebugServices>` plumbs `interaction_tx` and
//! `user_driver` between them; the readiness gate fires when the
//! frontend's `GeometryProvider` reports an element with
//! `has_content && entity_id.is_some()`.
//!
//! TUI deviations from GPUI (intentional, see plan §Architecture decisions):
//!
//! - Renderer drives `app_render` directly instead of going through
//!   `r3bl_tui::main_event_loop_impl`. Reason: r3bl's input device is a
//!   closed stream (`MockInputDevice` exhausts → loop breaks), and we
//!   need the renderer to keep producing frames as the engine fires
//!   CDC throughout the PBT. The watch task that the first
//!   `app_render` spawns sends
//!   `TerminalWindowMainThreadSignal::Render` through our channel; we
//!   loop on that signal to drive subsequent frames.
//! - Screenshots come from `OffscreenBufferBackend` painting the
//!   `OffscreenBuffer` we compose in `CapturingApp::app_render`, not
//!   from xcap. Same RGBA8 contract — `analyze_screenshot_emptiness`
//!   sees content when any cell has a non-blank glyph with a bright
//!   foreground color.
//!
//! `harness = false`. Run with: `cargo test -p holon-tui --test tui_ui_pbt`.

use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::{FrontendSession, ReactiveViewModel};
use holon_integration_tests::pbt::phased::{
    run_pbt_with_driver_sync_callback, PbtReadyContext, PbtReadyResult,
};
use holon_integration_tests::pbt::ui_harness::{
    screenshot_dir, set_memory_multiplier_if_unset, try_start_embedded_mcp, wait_for_geometry_ready,
};
use holon_integration_tests::ui_driver::VisualState;
use holon_integration_tests::GeometryDriver;
use holon_mcp::server::DebugServices;
use holon_tui::app_main::{AppSignal, TuiState, NO_FOCUS};
use holon_tui::geometry::{TuiGeometry, CELL_H, CELL_W};
use holon_tui::input_pump::setup_interaction_pump;
use holon_tui::render::RenderRegistry;
use holon_tui::user_driver::TuiUserDriver;
use r3bl_tui::test_fixtures::OutputDeviceExt;
use r3bl_tui::{
    height, width, App, ComponentRegistryMap, GlobalData, HasFocus, OffscreenBuffer,
    OffscreenBufferPool, OutputDevice, Size, TerminalWindowMainThreadSignal,
};

mod common;

use common::screenshot::OffscreenBufferBackend;
use common::test_harness::CapturingApp;

const LABEL: &str = "tui_ui_pbt";

/// DI context handed from the PBT thread to the main thread to construct
/// the TUI renderer. Mirrors GPUI's `GpuiLaunchContext`.
struct TuiLaunchContext {
    session: Arc<FrontendSession>,
    reactive_engine: Arc<ReactiveEngine>,
    runtime_handle: tokio::runtime::Handle,
}

fn main() {
    set_memory_multiplier_if_unset("15");

    let (ctx_tx, ctx_rx) = sync_channel::<TuiLaunchContext>(1);
    let (window_ready_tx, window_ready_rx) = sync_channel::<()>(1);

    let geometry = TuiGeometry::new();
    let visual_state: VisualState = Arc::new(Mutex::new(None));
    let captured: Arc<RwLock<Option<OffscreenBuffer>>> = Arc::new(RwLock::new(None));
    let debug = Arc::new(DebugServices::default());

    let dir = screenshot_dir("tui");

    // PBT thread: same shape as gpui_ui_pbt.rs:92-145. After `on_ready`
    // hands the DI context to the main thread, the PBT blocks on
    // `window_ready_rx` so chord/click dispatch goes against a populated
    // geometry registry rather than the empty initial frame.
    let pbt_geometry = geometry.clone();
    let pbt_visual_state = visual_state.clone();
    let driver_geometry: Arc<dyn GeometryProvider> = Arc::new(geometry.clone());
    let driver_for_pbtresult = geometry.clone();
    let pbt_visual_state_for_pbtresult = visual_state.clone();
    let pbt_captured = captured.clone();
    let pbt_debug = debug.clone();
    let pbt_handle = thread::spawn(move || {
        let backend = OffscreenBufferBackend::new(pbt_captured, CELL_W, CELL_H);
        let mut driver = GeometryDriver::new(Box::new(pbt_geometry))
            .with_screenshots(Box::new(backend), dir.clone())
            .with_visual_state(pbt_visual_state);

        let _signal_watcher = driver.spawn_signal_watcher();

        let result =
            run_pbt_with_driver_sync_callback(50, &mut driver, |pbt_ctx: &PbtReadyContext| {
                ctx_tx
                    .send(TuiLaunchContext {
                        session: pbt_ctx.session.clone(),
                        reactive_engine: pbt_ctx.reactive_engine.clone(),
                        runtime_handle: pbt_ctx.runtime_handle.clone(),
                    })
                    .expect("failed to send TuiLaunchContext to main thread");

                eprintln!("[{LABEL}] PBT waiting for TUI to be ready...");
                window_ready_rx
                    .recv_timeout(Duration::from_secs(120))
                    .expect("timed out waiting for TUI to become ready");
                eprintln!("[{LABEL}] TUI ready, continuing PBT steps");

                let tx = pbt_debug
                    .interaction_tx
                    .get()
                    .expect(
                        "interaction_tx not populated after window_ready — \
                         setup_interaction_pump should have run by now",
                    )
                    .clone();
                let tui_driver = TuiUserDriver::new(
                    tx,
                    driver_geometry.clone(),
                    pbt_ctx.reactive_engine.clone(),
                );
                Some(PbtReadyResult {
                    driver: Some(Box::new(tui_driver)),
                    frontend_engine: Some(pbt_ctx.reactive_engine.clone()),
                    frontend_geometry: Some(Box::new(driver_for_pbtresult.clone())),
                    frontend_visual_state: Some(pbt_visual_state_for_pbtresult.clone()),
                })
            });

        match result {
            Ok(summary) => {
                eprintln!("[{LABEL}] {summary}");
                eprintln!("[{LABEL}] screenshots at {}", dir.display());
            }
            Err(e) => {
                eprintln!("TUI UI PBT failed: {e:?}");
                std::process::exit(1);
            }
        }
    });

    // Main thread: wait for PBT-side DI context, then launch the TUI.
    let launch_ctx = ctx_rx
        .recv_timeout(Duration::from_secs(60))
        .expect("timed out waiting for TuiLaunchContext from PBT thread");

    try_start_embedded_mcp(
        &launch_ctx.runtime_handle,
        &launch_ctx.session,
        &launch_ctx.reactive_engine,
        "PBT_MCP_PORT",
        LABEL,
    );

    setup_interaction_pump(
        &debug,
        Arc::new(geometry.clone()),
        launch_ctx.reactive_engine.clone(),
        launch_ctx.runtime_handle.clone(),
    );

    // Spawn the readiness watcher BEFORE the renderer task starts —
    // mirrors gpui_ui_pbt.rs:209-214. Polls `geometry.all_elements()`
    // and signals `window_ready_tx` when the TUI has rendered an
    // element carrying both `has_content` and an `entity_id`.
    let ready_geometry: Arc<dyn GeometryProvider> = Arc::new(geometry.clone());
    thread::spawn(move || {
        wait_for_geometry_ready(&ready_geometry, Duration::from_secs(180), LABEL);
        let _ = window_ready_tx.send(());
        eprintln!("[{LABEL}] Window ready signal sent");
    });

    // Render driver runs as a task on the launch context's runtime. We
    // drive `app_render` ourselves rather than going through
    // `main_event_loop_impl` (see module-level note). The PBT thread
    // owns the only `Arc<Runtime>` (`phased.rs::create_runtime`), so
    // when the PBT exits and the runtime gets dropped, our task is
    // canceled mid-await. We don't wait on the task's completion (we
    // wait on the PBT thread's join below) — the runtime drop tears it
    // down whether or not it noticed `quit_rx`.
    let (_, quit_rx) = sync_channel::<()>(1); // unused — see comment above
    let runtime_handle = launch_ctx.runtime_handle.clone();
    let renderer_geometry = geometry.shared();
    let renderer_captured = captured.clone();
    runtime_handle.spawn(async move {
        run_capturing_renderer(launch_ctx, renderer_geometry, renderer_captured, quit_rx).await;
    });

    // Block the main thread on the PBT thread's join. This:
    //  - keeps the main thread OUT of the tokio runtime (no `block_on`
    //    that would panic when the runtime drops mid-await),
    //  - guarantees we observe the PBT's panic / Ok status before any
    //    runtime teardown work runs in main.
    let pbt_panicked = pbt_handle.join().is_err();
    if pbt_panicked {
        eprintln!("[{LABEL}] PBT thread panicked");
    }

    // The runtime is now dropping. Tasks (renderer, watch task,
    // optional MCP server) tear down concurrently with main's exit.
    // Bypass static dtors with explicit exit so any in-flight tokio
    // timer drop doesn't escalate to a process-wide panic.
    if pbt_panicked {
        eprintln!("[{LABEL}] exiting with PBT failure");
        std::process::exit(1);
    }
    eprintln!("[{LABEL}] exiting cleanly");
    std::process::exit(0);
}

/// Drive `CapturingApp::app_render` in a loop on the current runtime.
///
/// The first call spawns the watch task (via `ensure_watch_task_started`
/// inside `AppMain::app_render`) which sends
/// `TerminalWindowMainThreadSignal::Render` whenever the engine emits a
/// new `ReactiveViewModel`. We re-render on each signal; a 200 ms
/// fallback timer keeps the geometry registry warm during quiet periods
/// so the readiness gate doesn't stall waiting for an idle frontend.
async fn run_capturing_renderer(
    launch_ctx: TuiLaunchContext,
    last_registry: Arc<Mutex<RenderRegistry>>,
    captured: Arc<RwLock<Option<OffscreenBuffer>>>,
    quit_rx: std::sync::mpsc::Receiver<()>,
) {
    // 80 cols × 24 rows is a sensible default for a TUI under test;
    // CELL_W / CELL_H projects this to 640 × 384 px which clears
    // `analyze_screenshot_emptiness`'s `skip_y = 80` row strip.
    let initial_size: Size = width(80) + height(24);

    let (signal_tx, mut signal_rx) =
        tokio::sync::mpsc::channel::<TerminalWindowMainThreadSignal<AppSignal>>(64);

    let state = TuiState {
        session: launch_ctx.session.clone(),
        engine: launch_ctx.reactive_engine.clone(),
        rt_handle: launch_ctx.runtime_handle.clone(),
        status_message: "Ready".to_string(),
        current_model: Arc::new(Mutex::new(Arc::new(ReactiveViewModel::empty()))),
        watch_started: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        last_registry,
        focus_index: Arc::new(std::sync::atomic::AtomicUsize::new(NO_FOCUS)),
        focus_pin: Arc::new(Mutex::new(None)),
        edit_state: Arc::new(Mutex::new(None)),
        leader_pending: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };

    let (output_device, _stdout_mock) = OutputDevice::new_mock();
    let mut global = GlobalData::<TuiState, AppSignal>::try_to_create_instance(
        signal_tx,
        state,
        initial_size,
        output_device,
        OffscreenBufferPool::new(initial_size),
    )
    .expect("GlobalData construction failed");

    let mut registry: ComponentRegistryMap<TuiState, AppSignal> = ComponentRegistryMap::default();
    let mut focus = HasFocus::default();

    let mut app = CapturingApp::new(captured);
    app.app_init(&mut registry, &mut focus);

    // Initial render — spawns the watch task on the engine, so subsequent
    // CDC events flow back through `signal_rx` as `Render` signals.
    if let Err(e) = app.app_render(&mut global, &mut registry, &mut focus) {
        eprintln!("[{LABEL}] initial app_render failed: {e:?}");
        return;
    }

    loop {
        tokio::select! {
            biased;
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                // Liveness fallback — re-renders even when the engine
                // is idle so geometry stays current under inv14.
                if let Err(e) = app.app_render(&mut global, &mut registry, &mut focus) {
                    eprintln!("[{LABEL}] periodic app_render failed: {e:?}");
                    break;
                }
            }
            maybe_signal = signal_rx.recv() => {
                match maybe_signal {
                    Some(TerminalWindowMainThreadSignal::Render(_)) => {
                        if let Err(e) = app.app_render(&mut global, &mut registry, &mut focus) {
                            eprintln!("[{LABEL}] signal-driven app_render failed: {e:?}");
                            break;
                        }
                    }
                    Some(TerminalWindowMainThreadSignal::Exit) => break,
                    Some(TerminalWindowMainThreadSignal::ApplyAppSignal(_)) => {
                        // No app signals exercised by the TUI today; pass through.
                    }
                    None => break,
                }
            }
        }
        if quit_rx.try_recv().is_ok() {
            eprintln!("[{LABEL}] quit signal received, exiting renderer");
            break;
        }
    }
}
