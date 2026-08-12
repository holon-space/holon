//! Shared per-case windowed `ComposedSut<WideE2E>` boot/teardown helper.
//!
//! Extracted from `gpui_compose_sut_windowed.rs` (increment 3b) so the windowed
//! generated-sequence proptest loop (`gpui_composed_windowed_loop.rs`,
//! increment 4b) and the hand-built 3b tests share ONE copy of the intricate
//! gpui-thread window setup + `SimUserDriver` construction + leak-on-teardown
//! contract. This is deletion-scheduled scaffolding (Phase 1/2), so keeping a
//! single source avoids two copies drifting.

use std::panic::AssertUnwindSafe;
use std::panic::catch_unwind;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use gpui::PlatformTextSystem;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::user_driver::UserDriver;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::ReferenceState;
use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::harness::SettleHook;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2EMachine;
use holon_integration_tests::pbt::composed::wide_e2e::WideHandle;
use holon_integration_tests::pbt::composed::wide_e2e::boot_and_seed_wide_windowed_base;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::composed::wide_e2e::windowed_composed_sut;
use holon_integration_tests::pbt::fixtures::NamedFixture;
use holon_integration_tests::pbt::fixtures::replay_steps;
use holon_integration_tests::pbt::op_write_cap::IdResolver;
use holon_integration_tests::pbt::window_slice::builders::WindowMountConvention;
use holon_integration_tests::pbt::window_slice::builders::overlay_windowed_caps;

use super::capture::FrameSink;
use super::sim_windowed_replay::SimUserDriver;

pub fn real_text_system() -> Arc<dyn PlatformTextSystem> {
    gpui_platform::current_platform(true).text_system()
}

/// Cross-runtime fixed-point settle (the proven `gpui_window_slice` pattern):
/// pump until the element count is stable and no `"loading"` placeholders
/// remain.
pub fn settle_to_fixed_point(
    app: &mut HeadlessAppContext,
    bounds: &BoundsRegistry,
    runtime: &tokio::runtime::Runtime,
    timeout: Duration,
) {
    let start = Instant::now();
    let mut last_count = 0usize;
    let mut stable_iters = 0u32;
    while start.elapsed() < timeout {
        runtime.block_on(async { tokio::time::sleep(Duration::from_millis(20)).await });
        app.run_until_parked();
        app.advance_clock(Duration::from_secs(1));
        app.run_until_parked();
        bounds.flush();
        let elements = bounds.all_elements();
        let count = elements.len();
        let still_loading = elements
            .iter()
            .any(|(_, info)| info.widget_type.as_ref() == "loading");
        if count == last_count && count > 0 && !still_loading {
            stable_iters += 1;
            if stable_iters >= 5 {
                return;
            }
        } else {
            stable_iters = 0;
        }
        last_count = count;
    }
    panic!(
        "window never reached a fixed point within {timeout:?}: {} elements",
        bounds.all_elements().len()
    );
}

/// `*const HeadlessAppContext` behind a `Send` newtype so the window-settle
/// closure (a `SettleHook = Box<dyn Fn() + Send>`) can hold it. SAFETY: the
/// closure is only ever called on the gpui thread that owns the window, and
/// `app` is pinned for the driver's lifetime — the same single-thread contract
/// `SimUserDriver` relies on.
struct SendApp(*const HeadlessAppContext);
unsafe impl Send for SendApp {}
impl SendApp {
    // A `&self` accessor so a `move` closure captures the whole `SendApp` (Send)
    // rather than disjoint-capturing the raw-pointer field (2021 edition),
    // which would be !Send.
    fn app(&self) -> &HeadlessAppContext {
        unsafe { &*self.0 }
    }
}

/// Boot the wide-seeded windowed `ComposedSut<WideE2E>` (a gpui window renderer
/// over a `compose_sut` session + the wide-seeded backend caps + the window's
/// `SimUserDriver` gesture caps, assembled by `overlay_windowed_caps`), hand it
/// to `run` to drive/check, then tear the window down. `app` stays pinned on
/// this frame for the whole call, so the `*const HeadlessAppContext` the settle
/// hook / `SimUserDriver` / `FrameSink` hold stays valid. `run` takes the SUT
/// by value and returns it (so it can call the by-value
/// `StateMachineTest::apply`). It also receives the harness's default
/// `wide_e2e_ref()` oracle — the windowed loop IGNORES that and supplies its
/// own windowed-cap-set oracle (same tree, narrower cap set), which is sound
/// because the seed tree is identical — and a `FrameSink` for opt-in pixel
/// capture (a no-op unless `HOLON_CAPTURE_DIR` is set).
///
/// `run` returns `Option<ComposedSut>`: `Some(sut)` is leaked on teardown
/// (avoids the gpui leak detector + a runtime drop in async context); `None`
/// means the caller already consumed/dropped the SUT (e.g. a caught panic
/// inside a `catch_unwind` drive loop), so teardown just leaks the app. ⚠
/// `--test-threads=1` mandatory (gpui not parallel-safe).
pub fn with_windowed_wide_sut(
    run: impl FnOnce(ComposedSut<WideE2E>, &ReferenceState, &FrameSink) -> Option<ComposedSut<WideE2E>>,
) {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let resolver: IdResolver = Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new()));
    let oracle = wide_e2e_ref();

    // Boot the WIDE-seeded, driver-DEFERRED base (session/reactive surfaced for the
    // window).
    let (bundle, scaffold) =
        runtime.block_on(async { boot_and_seed_wide_windowed_base(&resolver, &oracle).await });
    let session = bundle
        .session
        .clone()
        .expect("full_headless -> booted FrontendSession");
    let engine = bundle
        .reactive
        .clone()
        .expect("full_headless -> booted ReactiveEngine");

    // Attach the window over the booted session/reactive; Some(debug) so the
    // interaction pump populates interaction_tx for the SimUserDriver.
    let debug = Arc::new(holon_mcp::server::DebugServices::default());
    let bounds = BoundsRegistry::new();
    let nav = NavigationState::new();
    let rebind = app
        .update(|cx| {
            launch_holon_window_rebindable(
                session.clone(),
                engine.clone(),
                runtime.handle().clone(),
                nav,
                bounds.clone(),
                Some(debug.clone()),
                None,
                "Holon-ComposeSut-Windowed-Loop",
                cx,
            )
        })
        .expect("window opened over wide compose_sut session");

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    // Build the live windowed driver + overlay its gesture caps onto the wide base.
    let interaction_tx = debug
        .interaction_tx
        .get()
        .expect("interaction_tx set by the window interaction pump")
        .clone();
    let app_ptr: *const HeadlessAppContext = &app;
    let driver: Arc<dyn UserDriver> = Arc::new(SimUserDriver::new(
        app_ptr,
        rebind.window(),
        bounds.clone(),
        engine.clone(),
        runtime.handle().clone(),
        interaction_tx,
    ));
    let geometry: Box<dyn GeometryProvider> = Box::new(bounds.clone());
    let frontend = bundle
        .frontend
        .clone()
        .expect("full_headless → booted HeadlessFrontendComponent");
    // Capture the settle handle (engine + frontend) before `bundle.caps` is moved,
    // so the windowed per-apply settle converges CDC + Loro + org like the
    // headless path.
    let handle = WideHandle::from_bundle(&bundle);
    let overlaid = overlay_windowed_caps(
        bundle.caps,
        frontend,
        geometry,
        engine.clone(),
        driver,
        resolver.clone(),
        WindowMountConvention::PerBlockShell,
    );

    // The window-settle hook the ComposedSut pumps before each check (mirror sim
    // `pump_cycle`: real wall-clock time for backend watchers on their own worker
    // threads, drain gpui, fire fake timers, promote staged bounds — no
    // block_on, driver methods may already be inside a tokio context).
    let settle_app = SendApp(app_ptr);
    let settle_bounds = bounds.clone();
    let settle: SettleHook = Box::new(move || {
        let app = settle_app.app();
        let mut last = usize::MAX;
        let mut stable = 0u32;
        for _ in 0..500 {
            std::thread::sleep(Duration::from_millis(10));
            app.run_until_parked();
            app.advance_clock(Duration::from_millis(500));
            app.run_until_parked();
            settle_bounds.flush();
            let els = settle_bounds.all_elements();
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
        panic!("windowed settle hook never reached a fixed point");
    });

    // A dedicated runtime drives the apply/check leaf futures; the booted backend
    // keeps running on `runtime`'s worker threads (kept alive by the outer
    // Arc). The gpui thread is NOT runtime-entered, so `rt.block_on` inside the
    // harness is legal here.
    let composed_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("composed runtime");

    // Assemble the windowed SUT (the initial page-root focus is already established
    // on the base by `boot_and_seed_wide_windowed_base` via the engine's
    // `dispatch_intent_sync`).
    let sut = windowed_composed_sut(overlaid, handle, resolver, scaffold, composed_rt, settle);

    // Pixel-capture sink: enabled only when HOLON_CAPTURE_DIR is set. Created
    // here (after the window exists + has settled) and handed to `run` so
    // per-step capture has both the app context and the window handle.
    let sink = FrameSink::new(&app, rebind.window());

    // Hand the live SUT to the caller to drive + check, then take it back for
    // teardown.
    let sut = run(sut, &oracle, &sink);

    // Teardown (see the 3b tests): release window entities, shut down, leak the
    // !Send app
    // + the SUT (which transitively holds the session + the composed runtime) so no
    //   Drop runs
    // the gpui leak detector or shuts a runtime down in an async context. If the
    // caller already consumed the SUT (`None`), only the app is leaked.
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    if let Some(sut) = sut {
        std::mem::forget(sut);
    }
}

/// Whether a caught panic payload's message contains `needle` — the windowed
/// minimizer's failure-signature classifier (moved here from the deleted
/// `windowed_replay` rebind service).
pub fn payload_signature_match(payload: &(dyn std::any::Any + Send), needle: &str) -> bool {
    let msg = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("");
    msg.contains(needle)
}

/// Replay a post-boot fixture (capture JSON / `.feature` scenario) through a
/// freshly booted windowed `ComposedSut<WideE2E>` — the composed successor of
/// the phased driver-sync replay spine (increment 4c). The base is the
/// pre-booted, wide-seeded `full_headless` window (no `StartApp` / org-seed
/// ceremony — the wide seed IS the boot org), so fixtures must be POST-BOOT,
/// matching composed-keystone captures and re-authored `.feature`s; a fixture
/// that still encodes `StartApp`/seed steps fails loud at the precondition
/// assert inside `replay_steps`.
///
/// Returns `Err(panic payload)` on the first step/invariant failure so ddmin
/// oracles (the windowed minimizer) can classify the signature; replay binaries
/// re-raise it. ⚠ `--test-threads=1` semantics apply (one gpui window per
/// process at a time).
pub fn replay_fixture_windowed(
    label: &'static str,
    fixture: &NamedFixture,
) -> Result<(), Box<dyn std::any::Any + Send + 'static>> {
    let mut result: Result<(), Box<dyn std::any::Any + Send + 'static>> = Ok(());
    with_windowed_wide_sut(|sut, oracle, sink| {
        // The composed windowed base is full_headless-wired; a capture recorded under a
        // different wiring would replay against the wrong oracle — fail loud, don't
        // fake.
        if let Some(recorded) = &fixture.wiring {
            assert_eq!(
                *recorded, oracle.harness.wiring,
                "[{label}] fixture {:?} was recorded under wiring {recorded:?}, but the windowed \
                 composed base is fixed to {:?} — re-record or replay headless",
                fixture.name, oracle.harness.wiring
            );
        }
        // Boot frame: the wide-seeded window, settled and rendered.
        sink.capture_action("boot");
        match catch_unwind(AssertUnwindSafe(|| {
            replay_steps::<WideE2EMachine, ComposedSut<WideE2E>>(
                label,
                &fixture.steps,
                oracle.clone(),
                sut,
                |_| sink.capture_action("start"),
                |_, _| sink.capture_pass("step"),
                None,
            )
        })) {
            Ok(sut) => Some(sut),
            Err(payload) => {
                // One failure frame before re-raising, so the flipbook ends on the
                // frame that actually failed. The SUT was consumed and dropped inside
                // the unwind (this thread is not runtime-entered, so its Drop is
                // safe); teardown only leaks the app.
                sink.capture_fail("failure", "");
                result = Err(payload);
                None
            }
        }
    });
    result
}
