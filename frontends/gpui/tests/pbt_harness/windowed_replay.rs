//! Reusable windowed-replay service: ONE GPUI window re-pointed at a fresh SUT
//! per replayed candidate via [`holon_gpui::RebindHandle`].
//!
//! This is the window-service half of `gpui_windowed_minimize.rs`, lifted out so
//! both the ddmin *minimizer* (replays captured candidates) and the proptest
//! *shrinker* (`gpui_ui_pbt`, replays generated + shrunk candidates) drive the
//! same plumbing. The architecture is the bg-thread-drives / main-thread-window
//! split:
//!
//!   - **main thread** runs `Application::run` ([`WindowHost::run_window`]):
//!     opens the window for the first candidate, then a rebind loop re-points it
//!     for every subsequent one and quits when the bg closure returns.
//!   - **bg thread** runs the caller's loop, replaying each candidate via
//!     [`WindowedReplayer::replay`]; each replay's `on_ready` posts a rebind
//!     request to the main thread, blocks until the window has repainted, then
//!     injects a `GpuiUserDriver`.
//!
//! State isolation is intact: every candidate builds its own `E2ESut` (fresh
//! Turso + Loro) inside `replay_fixture_with_driver_sync_callback` and runs
//! `StartApp` from empty — the window is re-pointed, not the backend reset, so
//! there is no cross-candidate poisoning.

#![allow(dead_code)]

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::Application;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::user_driver::UserDriver;
use holon_frontend::FrontendSession;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::navigation_state::NavigationState;
use holon_gpui::user_driver::GpuiUserDriver;
use holon_integration_tests::pbt::fixtures::FixtureStep;
use holon_integration_tests::pbt::phased::{
    replay_fixture_with_driver_sync_callback, PbtReadyContext, PbtReadyResult,
};
use holon_integration_tests::ui_driver::VisualState;
use holon_mcp::server::DebugServices;

/// What the bg thread asks the main thread to bind the window to.
struct RebindReq {
    session: Arc<FrontendSession>,
    engine: Arc<ReactiveEngine>,
}

/// Build the shared window-lifetime state and the bg↔main channels, returning
/// the main-thread [`WindowHost`] and the bg-thread [`WindowedReplayer`].
pub fn windowed_replay_service() -> (WindowHost, WindowedReplayer) {
    // ONE debug (its `interaction_tx` is set by the window's pump and read by
    // every candidate's GpuiUserDriver), ONE BoundsRegistry (the window's
    // geometry, read by inv-displayed-text/widget), ONE visual state.
    let debug = Arc::new(DebugServices::default());
    let bounds = BoundsRegistry::new();
    let visual_state: VisualState = Arc::new(std::sync::Mutex::new(None));

    let (req_tx, req_rx) = mpsc::channel::<RebindReq>();
    let (ack_tx, ack_rx) = mpsc::channel::<u64>(); // carries the fresh paint gen
    let done = Arc::new(AtomicBool::new(false));

    let host = WindowHost {
        debug: debug.clone(),
        bounds: bounds.clone(),
        req_rx,
        ack_tx,
        done: done.clone(),
    };
    let replayer = WindowedReplayer {
        debug,
        bounds,
        visual_state,
        req_tx,
        ack_rx,
        done,
    };
    (host, replayer)
}

/// Bg-thread handle: replays candidates through the reused window.
pub struct WindowedReplayer {
    debug: Arc<DebugServices>,
    bounds: BoundsRegistry,
    visual_state: VisualState,
    req_tx: mpsc::Sender<RebindReq>,
    ack_rx: mpsc::Receiver<u64>,
    done: Arc<AtomicBool>,
}

impl WindowedReplayer {
    /// Replay `steps` in the reused window. Returns `Ok(())` if the replay ran
    /// without panicking, `Err(payload)` carrying the panic payload otherwise.
    /// `seen_counter`, when `Some`, is incremented per applied transition so the
    /// proptest shrinker knows which transitions the test actually reached.
    /// `wiring` must match what the sequence was generated under (captures
    /// record it in `Fixture.environment.wiring`).
    pub fn replay(
        &self,
        wiring: holon_pbt_core::Wiring,
        steps: Vec<FixtureStep>,
        seen_counter: Option<Arc<AtomicUsize>>,
    ) -> Result<(), Box<dyn std::any::Any + Send>> {
        let req_tx = self.req_tx.clone();
        let ack_rx = &self.ack_rx;
        let debug = self.debug.clone();
        let bounds = self.bounds.clone();
        let visual = self.visual_state.clone();
        let on_ready = move |ctx: &PbtReadyContext| -> Option<PbtReadyResult> {
            req_tx
                .send(RebindReq {
                    session: ctx.session.clone(),
                    engine: ctx.reactive_engine.clone(),
                })
                .expect("main thread gone");
            ack_rx
                .recv_timeout(Duration::from_secs(200))
                .expect("window rebind ack");
            let tx = debug
                .interaction_tx
                .get()
                .expect("interaction_tx not set by window pump")
                .clone();
            let geometry: Arc<dyn GeometryProvider> = Arc::new(bounds.clone());
            let driver: Arc<dyn UserDriver> = Arc::new(GpuiUserDriver::new(
                tx,
                geometry,
                ctx.reactive_engine.clone(),
            ));
            Some(PbtReadyResult {
                driver: Some(driver),
                frontend_engine: Some(ctx.reactive_engine.clone()),
                frontend_geometry: Some(Box::new(bounds.clone())),
                frontend_visual_state: Some(visual.clone()),
            })
        };
        catch_unwind(AssertUnwindSafe(|| {
            // A setup error (not an invariant panic) is a hard failure, not a
            // "did not reproduce" — surface it loudly as a panic so the caller's
            // signature check rejects it rather than silently swallowing.
            replay_fixture_with_driver_sync_callback(
                wiring,
                steps,
                on_ready,
                |_, _| {},
                seen_counter,
            )
            .expect("windowed replay setup failed");
        }))
    }
}

/// Main-thread handle: owns the window for its lifetime.
pub struct WindowHost {
    debug: Arc<DebugServices>,
    bounds: BoundsRegistry,
    req_rx: mpsc::Receiver<RebindReq>,
    ack_tx: mpsc::Sender<u64>,
    done: Arc<AtomicBool>,
}

impl WindowHost {
    /// Spawn `bg` on a background thread, then run the GPUI `Application` on the
    /// current (main) thread: open the window for the first candidate and rebind
    /// it for each subsequent one until `bg` returns. Re-raises any panic `bg`
    /// produced once the window has quit.
    pub fn run_window(self, title: &str, bg: impl FnOnce() + Send + 'static) {
        let WindowHost {
            debug,
            bounds,
            req_rx,
            ack_tx,
            done,
        } = self;

        // Run `bg` under catch_unwind so a bg panic still releases the window
        // (done is set unconditionally), then re-raise it after the app quits.
        // The panic message is ALSO stashed in `bg_failure` before `done` flips:
        // on macOS `cx.quit()` terminates the process (exit 0) before
        // `bg_handle.join()` below ever runs, so the quit path must read the
        // failure itself and exit non-zero — otherwise CI sees a green run.
        let bg_failure: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let bg_done = done.clone();
        let bg_failure_writer = bg_failure.clone();
        let bg_handle = std::thread::spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(bg));
            if let Err(payload) = &result {
                *bg_failure_writer.lock().expect("bg_failure poisoned") =
                    Some(super::panic_message(payload));
            }
            bg_done.store(true, Ordering::SeqCst);
            result
        });

        // Block for the first candidate's rebind request before opening anything.
        let first = req_rx
            .recv_timeout(Duration::from_secs(120))
            .expect("first rebind request from bg thread");

        // A single window-lifetime runtime for the window's own spawns. Each
        // candidate's SUT owns its own runtime (created inside the replay) that
        // dies when its replay returns; the window must NOT borrow that, so it
        // gets this persistent one. The window only reads the SUT engine's (sync)
        // reactive snapshots, so a separate runtime for window-side tasks is fine.
        let window_rt =
            tokio::runtime::Runtime::new().expect("failed to build window-lifetime runtime");
        let window_rt_handle = window_rt.handle().clone();

        let app = Application::with_platform(gpui_platform::current_platform(false));
        let title = title.to_string();
        app.run(move |cx| {
            cx.activate(true);
            let nav = NavigationState::new();
            let handle = holon_gpui::launch_holon_window_rebindable(
                first.session,
                first.engine,
                window_rt_handle.clone(),
                nav,
                bounds.clone(),
                Some(debug.clone()),
                &title,
                cx,
            );
            let handle = match handle {
                Some(h) => h,
                None => {
                    eprintln!("[windowed-replay] window failed to open");
                    std::process::exit(1);
                }
            };

            // First candidate: wait for the initial paint, then ack.
            {
                let bounds = bounds.clone();
                let ack = ack_tx.clone();
                std::thread::spawn(move || {
                    let gen = wait_for_paint_quiescence(&bounds, 0, Duration::from_secs(180));
                    let _ = ack.send(gen);
                });
            }

            // Rebind loop for candidates 2..N; quit when the bg closure is done.
            cx.spawn(async move |cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(100))
                        .await;
                    if done.load(Ordering::SeqCst) {
                        // `cx.quit()` may terminate the process directly (macOS
                        // NSApp terminate → exit 0), so a bg failure must exit
                        // non-zero HERE, not after `app.run` returns.
                        if let Some(msg) = bg_failure.lock().expect("bg_failure poisoned").take() {
                            eprintln!("[windowed-replay] bg thread failed:\n{msg}");
                            std::process::exit(101);
                        }
                        let _ = cx.update(|cx| cx.quit());
                        break;
                    }
                    // NOTE: do NOT add an unconditional frame pump here. It was
                    // tried (2026-06-11) and broke per-keystroke pacing: typing
                    // paces on "one committed frame per key", and pump frames
                    // satisfy that wait before the editor echo lands — dropping
                    // characters (inv-displayed-text "#ir" vs "#+ir"). Waits
                    // that need a frame force one via the ScrollEntityIntoView
                    // RPC (its handler calls `window.refresh()`); the
                    // wait_for_entity_bounds timeout dump prints a frame-
                    // generation diagnosis if frame starvation ever recurs.
                    if let Ok(req) = req_rx.try_recv() {
                        let prev_gen = bounds.committed_generation();
                        let _ = cx.update(|cx| handle.rebind(req.session, req.engine, cx));
                        let bounds = bounds.clone();
                        let ack = ack_tx.clone();
                        std::thread::spawn(move || {
                            let gen = wait_for_paint_quiescence(
                                &bounds,
                                prev_gen,
                                Duration::from_secs(180),
                            );
                            let _ = ack.send(gen);
                        });
                    }
                }
                Ok::<_, anyhow::Error>(())
            })
            .detach();
        });

        // Window has quit. Re-raise a bg panic so the test process fails.
        match bg_handle.join() {
            Ok(Ok(())) => {}
            Ok(Err(payload)) => std::panic::resume_unwind(payload),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }
}

/// Block until the rebound window has *fully* repainted: a committed frame newer
/// than `prev_gen` AND the element count has been stable for several consecutive
/// polls (the reactive cascade — root → live blocks → their children — has
/// settled, so every entity a candidate's `NavigateFocus` needs is in the
/// BoundsRegistry). Waiting for only one fresh frame is not enough: deep trees
/// paint over multiple passes and a candidate that runs too early times out on
/// `wait_for_entity_bounds`. Returns the settled generation (or whatever it
/// reached at `timeout`).
fn wait_for_paint_quiescence(bounds: &BoundsRegistry, prev_gen: u64, timeout: Duration) -> u64 {
    let start = Instant::now();
    // Element count must hold still across this much wall time — frames may
    // keep committing (cursor blink), so stability is time-based, sampled at
    // a fast cadence. 240ms covers the multi-pass cascade (root → live
    // blocks → children) without the old 4×120ms ≈ 0.5s floor.
    let stable_window = Duration::from_millis(240);
    let mut last_count = usize::MAX;
    let mut stable_since: Option<Instant> = None;
    loop {
        bounds.flush();
        let gen = bounds.committed_generation();
        let count = bounds.all_elements().len();
        if gen > prev_gen && count > 0 && count == last_count {
            let since = *stable_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= stable_window {
                return gen;
            }
        } else {
            stable_since = None;
        }
        last_count = count;
        if start.elapsed() >= timeout {
            eprintln!(
                "[windowed-replay] paint quiescence timeout (gen {gen} vs prev {prev_gen}, \
                 {count} elements) — proceeding anyway"
            );
            return gen;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

/// True iff the caught panic payload's message contains `needle` — the
/// signature guard both the ddmin minimizer and the proptest shrinker use to
/// avoid collapsing into a *different* failure mode.
pub fn payload_signature_match(payload: &(dyn std::any::Any + Send), needle: &str) -> bool {
    let msg = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("");
    msg.contains(needle)
}
