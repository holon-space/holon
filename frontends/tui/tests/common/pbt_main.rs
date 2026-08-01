//! Shared body of the TUI composed windowed random runner (increment 4c).
//!
//! Repointed off the phased driver-sync generator spine onto the
//! composed windowed path: boot the wide-seeded `ComposedSut<WideE2E>` base
//! (`boot_and_seed_wide_windowed_base` — the same `compose_sut(full_headless)`
//! session the gpui windowed loop rides), attach the TUI capturing renderer
//! over its `FrontendSession`/`ReactiveEngine`, overlay the
//! `TuiUserDriver`-backed gesture caps (`overlay_windowed_caps`), then drive
//! ONE generated `E2ETransition` sequence from the windowed alphabet
//! (`WideE2EWindowedMachine`, narrowed live cap set) with the full composed
//! catalog checked every tick (`ComposedSut::check_invariants`).
//!
//! Deviations from the gpui 4b loop, DISCLOSED:
//! - ONE boot + ONE sequence per process (no per-case reboot, no in-process
//!   shrinking): the TUI renderer task owns process-wide channels, so the
//!   deterministic reproduction knob is `PROPTEST_SEED`, and the shrinker home
//!   remains the gpui loop / headless keystone (same alphabet, same catalog).
//! - No screenshot pipeline (that belonged to the phased `GeometryDriver`
//!   registry); the `screenshot_painter` target still exercises the
//!   `OffscreenBufferBackend`.
//! - Wiring is fixed to `full_headless` (the composed windowed base); the
//!   `sql_only` twin died with the phased generator (wiring parameterization
//!   returns with the env-selected ONE PBT, Phase 4).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use holon_frontend::FrontendSession;
use holon_frontend::ReactiveViewModel;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::user_driver::UserDriver;
use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::harness::SettleHook;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2EMachine;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2EWindowedMachine;
use holon_integration_tests::pbt::composed::wide_e2e::WideHandle;
use holon_integration_tests::pbt::composed::wide_e2e::boot_and_seed_wide_windowed_base;
use holon_integration_tests::pbt::composed::wide_e2e::disclose_excluded;
use holon_integration_tests::pbt::composed::wide_e2e::narrow_to_windowed_alphabet;
use holon_integration_tests::pbt::composed::wide_e2e::set_windowed_cap_set;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::composed::wide_e2e::windowed_composed_sut;
use holon_integration_tests::pbt::op_write_cap::IdResolver;
use holon_integration_tests::pbt::ui_harness::try_start_embedded_mcp;
use holon_integration_tests::pbt::ui_harness::wait_for_geometry_ready;
use holon_integration_tests::pbt::window_slice::builders::WindowMountConvention;
use holon_integration_tests::pbt::window_slice::builders::overlay_windowed_caps;
use holon_mcp::server::DebugServices;
use holon_tui::app_main::AppSignal;
use holon_tui::app_main::EditState;
use holon_tui::app_main::NO_FOCUS;
use holon_tui::app_main::TuiState;
use holon_tui::geometry::TuiGeometry;
use holon_tui::input_pump::setup_interaction_pump;
use holon_tui::render::RenderRegistry;
use proptest::strategy::Strategy;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;
use r3bl_tui::App;
use r3bl_tui::ComponentRegistryMap;
use r3bl_tui::GlobalData;
use r3bl_tui::HasFocus;
use r3bl_tui::InputEvent;
use r3bl_tui::OffscreenBuffer;
use r3bl_tui::OffscreenBufferPool;
use r3bl_tui::OutputDevice;
use r3bl_tui::Size;
use r3bl_tui::TerminalWindowMainThreadSignal;
use r3bl_tui::height;
use r3bl_tui::test_fixtures::OutputDeviceExt;
use r3bl_tui::width;

use super::test_harness::CapturingApp;

/// DI context handed to the renderer task. Mirrors GPUI's launch context.
struct TuiLaunchContext {
    engine: Arc<holon::api::BackendEngine>,
    session: Arc<FrontendSession>,
    reactive_engine: Arc<ReactiveEngine>,
    runtime_handle: tokio::runtime::Handle,
    debug_services: Arc<holon_mcp::server::DebugServices>,
}

/// Boot the composed wide windowed base, attach the TUI renderer, overlay the
/// `TuiUserDriver` gesture caps, and drive one generated windowed sequence with
/// the composed catalog checked every tick. Panics loud on the first
/// divergence.
pub fn run(label: &'static str) {
    // ── 1. Composed wide base (backend + windowless
    // FrontendSession/ReactiveEngine). ──
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
    let oracle = wide_e2e_ref();
    let (bundle, scaffold) =
        runtime.block_on(async { boot_and_seed_wide_windowed_base(&resolver, &oracle).await });
    let session = bundle
        .session
        .clone()
        .expect("full_headless -> booted FrontendSession");
    let reactive = bundle
        .reactive
        .clone()
        .expect("full_headless -> booted ReactiveEngine");
    let backend_engine = bundle
        .engine
        .clone()
        .expect("full_headless -> booted Turso BackendEngine");
    let frontend = bundle
        .frontend
        .clone()
        .expect("full_headless -> booted HeadlessFrontendComponent");

    // ── 2. TUI renderer plumbing (the same lifted Arcs the phased harness used —
    // see frontends/tui/src/user_driver.rs module docs for why they are shared). ──
    let debug = Arc::new(DebugServices::default());
    let captured: Arc<RwLock<Option<OffscreenBuffer>>> = Arc::new(RwLock::new(None));
    let geometry = TuiGeometry::new();
    let last_registry: Arc<Mutex<RenderRegistry>> = geometry.shared();
    let focus_index: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(NO_FOCUS));
    let edit_state: Arc<Mutex<Option<EditState>>> = Arc::new(Mutex::new(None));
    let leader_pending: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    let render_seq: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
    let render_notify: Arc<tokio::sync::Notify> = Arc::new(tokio::sync::Notify::new());
    let (input_tx, input_rx) = tokio::sync::mpsc::channel::<InputEvent>(64);

    let type_registry = runtime.block_on(frontend.type_registry());
    try_start_embedded_mcp(
        runtime.handle(),
        &backend_engine,
        &reactive,
        type_registry,
        debug.clone(),
        "PBT_MCP_PORT",
        label,
    );

    setup_interaction_pump(
        &debug,
        Arc::new(geometry.clone()),
        reactive.clone(),
        runtime.handle().clone(),
        input_tx.clone(),
        last_registry.clone(),
        focus_index.clone(),
        edit_state.clone(),
        render_seq.clone(),
        render_notify.clone(),
    );

    // ── 3. Renderer task over the booted session/reactive (self-driving; the
    // settle hook below only POLLS geometry — the renderer keeps producing
    // frames on CDC). ──
    let launch_ctx = TuiLaunchContext {
        engine: backend_engine,
        session,
        reactive_engine: reactive.clone(),
        runtime_handle: runtime.handle().clone(),
        debug_services: debug.clone(),
    };
    let (_quit_tx, quit_rx) = std::sync::mpsc::sync_channel::<()>(1);
    let renderer_geometry = geometry.clone();
    let renderer_captured = captured.clone();
    let renderer_focus_index = focus_index.clone();
    let renderer_edit_state = edit_state.clone();
    let renderer_leader_pending = leader_pending.clone();
    let renderer_render_seq = render_seq.clone();
    let renderer_render_notify = render_notify.clone();
    runtime.handle().spawn(async move {
        run_capturing_renderer(
            label,
            launch_ctx,
            renderer_geometry,
            renderer_focus_index,
            renderer_edit_state,
            renderer_leader_pending,
            renderer_captured,
            input_rx,
            renderer_render_seq,
            renderer_render_notify,
            quit_rx,
        )
        .await;
    });

    let ready_geometry: Arc<dyn GeometryProvider> = Arc::new(geometry.clone());
    wait_for_geometry_ready(&ready_geometry, Duration::from_secs(180), label);

    // ── 4. TuiUserDriver + windowed cap overlay (the TUI sibling of the gpui
    // `SimUserDriver` overlay in `windowed_wide.rs`). ──
    let interaction_tx = debug
        .interaction_tx
        .get()
        .expect("interaction_tx set by setup_interaction_pump")
        .clone();
    let driver_geometry: Arc<dyn GeometryProvider> = Arc::new(geometry.clone());
    let tui_driver: Arc<dyn UserDriver> = Arc::new(holon_tui::user_driver::TuiUserDriver::new(
        reactive.clone(),
        driver_geometry,
        input_tx.clone(),
        last_registry.clone(),
        focus_index.clone(),
        edit_state.clone(),
        render_seq.clone(),
        render_notify.clone(),
        interaction_tx,
    ));
    let geometry_box: Box<dyn GeometryProvider> = Box::new(geometry.clone());
    // Settle handle (engine + frontend) captured before `bundle.caps` is moved, so
    // the per-apply settle converges CDC + Loro + org like the headless path.
    let handle = WideHandle::from_bundle(&bundle);
    let overlaid = overlay_windowed_caps(
        bundle.caps,
        frontend,
        geometry_box,
        reactive,
        tui_driver,
        resolver.clone(),
        WindowMountConvention::InlineRow,
    );

    // Settle hook: the renderer self-drives on the backend runtime, so settling is
    // pure polling — wait until the element count is stable and no "loading"
    // placeholders remain (the TUI mirror of the gpui fixed-point settle).
    let settle_geometry = geometry.clone();
    let settle: SettleHook = Box::new(move || {
        let mut last = usize::MAX;
        let mut stable = 0u32;
        for _ in 0..500 {
            std::thread::sleep(Duration::from_millis(10));
            let els = settle_geometry.all_elements();
            let count = els.len();
            let loading = els.iter().any(|(_, i)| i.widget_type.as_ref() == "loading");
            if count > 0 && count == last && !loading {
                stable += 1;
                if stable >= 3 {
                    return;
                }
            } else {
                stable = 0;
            }
            last = count;
        }
        panic!("[tui composed] settle hook never reached a fixed point");
    });

    // A dedicated runtime drives the apply/check leaf futures; the booted backend
    // keeps running on `runtime`'s worker threads. The main thread is NOT
    // runtime-entered, so the harness's internal `block_on` is legal here.
    let composed_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("composed runtime");
    let mut sut = windowed_composed_sut(overlaid, handle, resolver, scaffold, composed_rt, settle);

    // ── 5. ONE generated windowed sequence, composed catalog every tick. ──
    let live = sut.cap_set();
    disclose_excluded(&live);
    // TUI-SPECIFIC DISCLOSED EXCLUSION (tracked Phase-3 blocker, same doctrine as
    // the C-3 rows 19–24): the keystroke-backed block-tree rows
    // (SplitBlock/JoinBlock/ Indent/Outdent/Move*) ride
    // `KeystrokeBlockTreeWriter` (focus editor + Home/ Right×n/Enter/Tab/
    // Backspace through the driver), and the TUI's `app_handle_input_event`
    // editor rung does not implement those edits yet — the first generated
    // SplitBlock minted a ref block while the SUT created nothing. The class is
    // genuinely not TUI-driver-backed, so it must NOT enter the TUI
    // generated alphabet (an unfaithful rung combination would fabricate
    // divergences). The cap stays in the CapMap (its read invariants keep
    // selecting); only generation drops the rows. Re-admit once the TUI editor
    // rung is rebound.
    use holon_pbt_core::capabilities::SutBlockTreeWrite;
    use holon_pbt_core::composition::CapId;
    let tui_alphabet =
        narrow_to_windowed_alphabet(live).without(&CapId::of::<dyn SutBlockTreeWrite>());
    eprintln!(
        "[{label}] TUI EXCLUDED cap: SutBlockTreeWrite (Split/Join/Indent/Outdent/Move*) — \
         keystroke editor rung not TUI-backed yet (tracked Phase-3 blocker)"
    );
    set_windowed_cap_set(tui_alphabet);

    let num_steps: usize = std::env::var("PBT_NUM_STEPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    let mut sampler = TestRunner::default();
    let (initial_ref, transitions, _seen) =
        WideE2EWindowedMachine::sequential_strategy(1..=num_steps)
            .new_tree(&mut sampler)
            .expect("draw windowed sequence")
            .current();
    let kinds: Vec<&'static str> = transitions.iter().map(|t| t.variant_name()).collect();
    eprintln!(
        "[{label}] {} transition(s) drawn from the windowed alphabet: {kinds:?}",
        kinds.len()
    );

    let mut ref_state = initial_ref;
    ComposedSut::<WideE2E>::check_invariants(&sut, &ref_state);
    for (i, transition) in transitions.into_iter().enumerate() {
        eprintln!("[{label}] step {i}: {}", transition.variant_name());
        ref_state = <WideE2EMachine as ReferenceStateMachine>::apply(ref_state, &transition);
        sut = ComposedSut::<WideE2E>::apply(sut, &ref_state, transition);
        ComposedSut::<WideE2E>::check_invariants(&sut, &ref_state);
    }
    eprintln!(
        "[{label}] PASS — {} windowed step(s) GREEN over the TUI composed SUT (full catalog every \
         tick)",
        kinds.len()
    );

    // Teardown: leak the SUT (it owns the composed runtime + session) and exit
    // before the backend runtime drops mid-await in the renderer task.
    std::mem::forget(sut);
    std::process::exit(0);
}

/// Drive `CapturingApp::app_render` in a loop on the current runtime,
/// also draining synthetic `InputEvent`s from the PBT-side
/// `TuiUserDriver` and routing them through the real
/// `app_handle_input_event` path.
///
/// Three select arms (non-biased — fairness keeps inputs from being
/// starved by the periodic timer):
///
/// - 200 ms periodic re-render — liveness fallback for inv-frontend-engine
///   (keeps geometry warm during quiet periods, doubles as the deadlock
///   backstop for `EventPropagation::Propagate` events that don't trigger a
///   signal-driven render).
/// - `signal_rx` — `TerminalWindowMainThreadSignal::Render` from the engine
///   watch task, fired on every CDC emission.
/// - `input_rx` — synthetic `InputEvent`s from `TuiUserDriver`. The real
///   `app_handle_input_event` is invoked, then we re-render unconditionally so
///   the driver always has a barrier to wait on regardless of the returned
///   `EventPropagation`.
///
/// Every successful `app_render` bumps `render_seq` and notifies
/// `render_notify`, forming the lost-wakeup-safe render barrier the
/// driver uses for sequencing nav steps and chord settling.
#[allow(clippy::too_many_arguments)]
async fn run_capturing_renderer(
    label: &'static str,
    launch_ctx: TuiLaunchContext,
    geometry: TuiGeometry,
    focus_index: Arc<AtomicUsize>,
    edit_state: Arc<Mutex<Option<EditState>>>,
    leader_pending: Arc<AtomicBool>,
    captured: Arc<RwLock<Option<OffscreenBuffer>>>,
    mut input_rx: tokio::sync::mpsc::Receiver<InputEvent>,
    render_seq: Arc<AtomicU64>,
    render_notify: Arc<tokio::sync::Notify>,
    quit_rx: std::sync::mpsc::Receiver<()>,
) {
    // 80 cols × 24 rows is a sensible default for a TUI under test;
    // CELL_W / CELL_H projects this to 640 × 384 px which clears
    // `analyze_screenshot_emptiness`'s `skip_y = 80` row strip.
    let initial_size: Size = width(80) + height(24);

    let (signal_tx, mut signal_rx) =
        tokio::sync::mpsc::channel::<TerminalWindowMainThreadSignal<AppSignal>>(64);

    // TuiState consumes the passed-in Arcs (not freshly allocated) so
    // the driver and renderer share live focus / registry / edit state.
    let state = TuiState {
        session: launch_ctx.session.clone(),
        engine: launch_ctx.reactive_engine.clone(),
        rt_handle: launch_ctx.runtime_handle.clone(),
        status_message: "Ready".to_string(),
        current_model: Arc::new(Mutex::new(Arc::new(ReactiveViewModel::empty()))),
        watch_started: Arc::new(AtomicBool::new(false)),
        last_registry: geometry,
        focus_index,
        focus_pin: Arc::new(Mutex::new(None)),
        edit_state,
        leader_pending,
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
        eprintln!("[{label}] initial app_render failed: {e:?}");
        return;
    }
    render_seq.fetch_add(1, Ordering::Release);
    render_notify.notify_waiters();

    loop {
        tokio::select! {
            // No biased — fairness across the three arms keeps inputs
            // from being starved by the timer / signal arms.
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                // Liveness fallback — re-renders even when the engine
                // is idle so geometry stays current under inv-frontend-engine.
                if let Err(e) = app.app_render(&mut global, &mut registry, &mut focus) {
                    eprintln!("[{label}] periodic app_render failed: {e:?}");
                    break;
                }
                render_seq.fetch_add(1, Ordering::Release);
                render_notify.notify_waiters();
            }
            maybe_signal = signal_rx.recv() => {
                match maybe_signal {
                    Some(TerminalWindowMainThreadSignal::Render(_)) => {
                        if let Err(e) = app.app_render(&mut global, &mut registry, &mut focus) {
                            eprintln!("[{label}] signal-driven app_render failed: {e:?}");
                            break;
                        }
                        render_seq.fetch_add(1, Ordering::Release);
                        render_notify.notify_waiters();
                    }
                    Some(TerminalWindowMainThreadSignal::Exit) => break,
                    Some(TerminalWindowMainThreadSignal::ApplyAppSignal(_)) => {
                        // No app signals exercised by the TUI today; pass through.
                    }
                    None => break,
                }
            }
            maybe_input = input_rx.recv() => {
                let Some(ev) = maybe_input else { break };
                // Reuse `app: CapturingApp` (which proxies to AppMain via
                // tests/common/test_harness.rs:48-57) — do NOT instantiate
                // a fresh AppMain. The propagation flag is informational;
                // we re-render unconditionally so the driver's
                // `await_render` barrier always fires regardless of
                // `EventPropagation::Propagate` vs `ConsumedRender`.
                let _propagation = match app.app_handle_input_event(
                    ev, &mut global, &mut registry, &mut focus,
                ) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("[{label}] app_handle_input_event failed: {e:?}");
                        break;
                    }
                };
                if let Err(e) = app.app_render(&mut global, &mut registry, &mut focus) {
                    eprintln!("[{label}] post-input app_render failed: {e:?}");
                    break;
                }
                render_seq.fetch_add(1, Ordering::Release);
                render_notify.notify_waiters();
            }
        }
        if quit_rx.try_recv().is_ok() {
            eprintln!("[{label}] quit signal received, exiting renderer");
            break;
        }
    }
}
