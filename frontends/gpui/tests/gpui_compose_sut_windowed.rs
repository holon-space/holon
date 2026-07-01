//! ★ Round-5 windowed repoint — foundational milestone.
//!
//! ✅ VERIFIED GREEN (macOS, 2026-07-01) — 2 passed:
//!   `cargo test -p holon-gpui --features pbt --test gpui_compose_sut_windowed -- --test-threads=1`
//!   1. window rendered 68 elements (63 non-degenerate) over the `compose_sut_windowed_base`
//!      session; base hosts `SutBackend` (13 blocks); driver rung deferred.
//!   2. `overlay_windowed_caps` (runtime-exercised) built a CapMap with `SutLayout` (68 elems) +
//!      `SutBackend` (13 blocks) + the window's `SutDriver`/`SutBlockInteract` over a live window.
//! ⚠ MUST run with `--test-threads=1`: gpui `TestApp` is not parallel-safe (thread-local platform
//! state); two windowed tests in one binary SIGABRT if run concurrently.
//!
//! Proves the claim the whole repoint rests on: a gpui window RENDERS a
//! [`compose_sut_windowed_base`] session (the window is a *pure renderer* over a
//! headless-booted `FrontendSession` + `ReactiveEngine`), and that deferred-driver
//! base already hosts the backend caps reading the booted store. Together with the
//! surfaced `session`/`reactive` handles, this shows the windowed CapMap can be
//! assembled by booting the headless composition and attaching a window over its
//! reactive engine — no separate booter, no new id-reconcile.
//!
//! What is DEFERRED to a later increment (increment 3): the faithful windowed
//! gesture driver (`SimUserDriver`, which needs a live gpui `App` pointer +
//! `interaction_tx`) and the full `overlay_windowed_caps` + `StateMachineTest`
//! per-tick loop with a matched reference oracle. This milestone deliberately reads
//! `SutLayout` (through `window_layout`) + `SutBackend` (through the base CapMap)
//! directly, so it needs no reference matching.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{AssetSource, PlatformTextSystem, TestApp};
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::user_driver::UserDriver;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::composed::builder::compose_sut_windowed_base;
use holon_integration_tests::pbt::composed::harness::{ComposedSut, SettleHook};
use holon_integration_tests::pbt::composed::wide_e2e::{
    boot_and_seed_wide_windowed_base, wide_e2e_ref, windowed_composed_sut, WideE2E,
};
use holon_integration_tests::pbt::op_write_cap::IdResolver;
use holon_integration_tests::pbt::window_slice::builders::{overlay_windowed_caps, window_layout};
use holon_pbt_core::ComponentSet;
use proptest_state_machine::StateMachineTest;
// Caps must be in scope to read them through the `CapMap` (capmap_adapter forwards).
use holon_pbt_core::capabilities::{SutBackend, SutBlockInteract, SutDriver, SutLayout};

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;
use pbt_harness::sim_windowed_replay::SimUserDriver;

fn real_text_system() -> Arc<dyn PlatformTextSystem> {
    gpui_platform::current_platform(true).text_system()
}

/// Cross-runtime fixed-point settle (the proven `gpui_window_slice` pattern): pump
/// until the element count is stable and no `"loading"` placeholders remain.
fn settle_to_fixed_point(
    app: &mut TestApp,
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

#[test]
fn window_renders_compose_sut_base_and_base_hosts_backend() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = TestApp::with_text_system_and_assets(text_system, assets);

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));

    // Boot the DEFERRED-driver headless base (`full_headless`): everything a wide
    // headless SUT has (backend / storage / editor / ViewModel caps + IdResolver
    // reconcile) EXCEPT the gesture-driver rung, which a window would supply.
    let resolver: IdResolver = Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new()));
    let composed = runtime.block_on(async {
        compose_sut_windowed_base(&ComponentSet::full_headless(), &resolver).await
    });

    let session = composed
        .session
        .clone()
        .expect("full_headless has ViewModel → a booted FrontendSession");
    let engine = composed
        .reactive
        .clone()
        .expect("full_headless has ViewModel → a booted frontend ReactiveEngine");

    // (1) Deferred-driver contract: the base carries NO driver rung, so a later
    //     `overlay_windowed_caps` INSERTS the window's driver caps as sole providers.
    assert!(
        composed.caps.get::<dyn SutDriver>().is_none(),
        "compose_sut_windowed_base must DEFER the driver rung (no SutDriver in the base)",
    );

    // (2) Backend caps present and reading the booted store (the boot seed doc).
    let booted_blocks = runtime.block_on(async { composed.caps.block_raw_snapshot().await });
    assert!(
        !booted_blocks.is_empty(),
        "the deferred base must host SutBackend reading the booted block_raw store",
    );

    // Attach a TestPlatform window over the SAME session + reactive engine — the
    // window is a pure renderer; no session construction of its own.
    let bounds = BoundsRegistry::new();
    let nav = NavigationState::new();
    let _rebind = app
        .update(|cx| {
            launch_holon_window_rebindable(
                session.clone(),
                engine.clone(),
                runtime.handle().clone(),
                nav,
                bounds.clone(),
                None,
                "Holon-ComposeSut-Windowed",
                cx,
            )
        })
        .expect("window opened over compose_sut session");

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    // (3) SutLayout over the window reads real, non-degenerate geometry — proving the
    //     window RENDERS compose_sut's session (the foundational Hard-B claim).
    let geometry: Box<dyn GeometryProvider> = Box::new(bounds.clone());
    let capmap = window_layout(geometry);
    let via_capmap = runtime.block_on(async { capmap.rendered_elements().await });
    assert!(
        !via_capmap.is_empty(),
        "a window over the compose_sut session produced no geometry",
    );
    let non_degenerate = via_capmap
        .iter()
        .filter(|e| e.width > 1.0 && e.height > 1.0)
        .count();
    assert!(
        non_degenerate >= 1,
        "the compose_sut window produced only degenerate geometry",
    );

    eprintln!(
        "[compose_sut-windowed] PASS — window rendered {} elements ({non_degenerate} non-degenerate) \
         over a compose_sut_windowed_base session; base hosts SutBackend ({} booted blocks); driver deferred",
        via_capmap.len(),
        booted_blocks.len(),
    );

    // gpui teardown (mirror `gpui_window_slice.rs`): release the window entities, shut the
    // app down, then leak the `!Send` TestApp + the booted composition so their Drops don't
    // run the gpui leak detector / drop the session's tokio runtime in async context. The
    // process exits right after the test, so the leak is inert.
    drop(_rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(composed);
}

#[test]
fn overlay_windowed_caps_composes_layout_backend_and_driver_over_a_live_window() {
    // Increment-3 sub-step 3a: runtime-exercise `overlay_windowed_caps` (until now only
    // compile-verified). Onto the DEFERRED-driver `compose_sut_windowed_base` CapMap it must
    // INSERT the window's `SutLayout` geometry + the live `SimUserDriver`-backed gesture caps
    // while the base's `SutBackend` survives — the full windowed CapMap the StateMachineTest
    // runner (3b) will drive. Also de-risks the intricate `SimUserDriver` construction over a
    // compose_sut window.
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = TestApp::with_text_system_and_assets(text_system, assets);

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let resolver: IdResolver = Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new()));
    let composed = runtime.block_on(async {
        compose_sut_windowed_base(&ComponentSet::full_headless(), &resolver).await
    });

    let session = composed
        .session
        .clone()
        .expect("full_headless → booted FrontendSession");
    let engine = composed
        .reactive
        .clone()
        .expect("full_headless → booted frontend ReactiveEngine");

    // The window populates this `DebugServices`' `interaction_tx` once up; the
    // `SimUserDriver` drives real platform input through it.
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
                "Holon-ComposeSut-Overlay",
                cx,
            )
        })
        .expect("window opened over compose_sut session");

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    // The same real windowed driver the windowed PBT loop uses.
    let interaction_tx = debug
        .interaction_tx
        .get()
        .expect("interaction_tx set by the window interaction pump")
        .clone();
    let app_ptr: *const TestApp = &app;
    let driver: Arc<dyn UserDriver> = Arc::new(SimUserDriver::new(
        app_ptr,
        rebind.window(),
        bounds.clone(),
        engine.clone(),
        runtime.handle().clone(),
        interaction_tx,
    ));

    // ★ Exercise the pure-insert overlay at runtime. Its internal fail-loud assert also
    // confirms the base DEFERRED its driver (no SutDriver present) before inserting.
    let geometry: Box<dyn GeometryProvider> = Box::new(bounds.clone());
    let overlaid = overlay_windowed_caps(composed.caps, geometry, engine.clone(), driver);

    // (1) The overlay INSERTED the window driver rung (absent in the deferred base).
    assert!(
        overlaid.get::<dyn SutDriver>().is_some(),
        "overlay_windowed_caps must INSERT the window SutDriver",
    );
    assert!(
        overlaid.get::<dyn SutBlockInteract>().is_some(),
        "overlay_windowed_caps must INSERT the window SutBlockInteract gesture cap",
    );
    // (2) SutLayout reads real geometry through the overlaid CapMap (window renders it).
    let elems = runtime.block_on(async { overlaid.rendered_elements().await });
    assert!(
        !elems.is_empty(),
        "overlaid CapMap's SutLayout returned no geometry",
    );
    // (3) The base's SutBackend survived the overlay (still reads the booted store).
    let blocks = runtime.block_on(async { overlaid.block_raw_snapshot().await });
    assert!(
        !blocks.is_empty(),
        "overlaid CapMap lost the base SutBackend",
    );

    eprintln!(
        "[compose_sut-overlay] PASS — overlay_windowed_caps built a CapMap with SutLayout ({} elems),          SutBackend ({} blocks), and the window's SutDriver + SutBlockInteract over a live window",
        elems.len(),
        blocks.len(),
    );

    // gpui teardown (see the foundational test): release window entities, shut down, then
    // leak the `!Send` app + the overlaid caps (which transitively hold the session) so no
    // Drop runs the leak detector or drops the session's runtime in async context.
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(overlaid);
}

#[test]
fn windowed_composed_sut_runs_full_catalog_green_on_the_initial_frame() {
    // ★ Increment 3b (sub-step i): the windowed StateMachineTest runner's foundational
    // check. Assemble a `ComposedSut<WideE2E>` around the OVERLAID windowed caps (window +
    // wide-seeded backend), then run the UNIFIED composed catalog through the real
    // `StateMachineTest::check_invariants` over the initial rendered frame. This is what
    // `replay_steps`/proptest will call per tick — proving the block/storage families AND
    // the windowed geometry family run GREEN against ONE `wide_e2e_ref()` oracle in a
    // single SUT (the repoint's whole point: one SUT, not E2ESut + a parallel windowed
    // check).
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = TestApp::with_text_system_and_assets(text_system, assets);

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let resolver: IdResolver = Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new()));

    // The oracle the SUT is measured against — the wide working tree focused on the page.
    let oracle = wide_e2e_ref();

    // Boot the WIDE-seeded, driver-DEFERRED base (session/reactive surfaced for the window).
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

    // Attach the window over the booted session/reactive; Some(debug) so the interaction
    // pump populates interaction_tx for the SimUserDriver.
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
                "Holon-ComposeSut-Windowed-3b",
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
    let app_ptr: *const TestApp = &app;
    let driver: Arc<dyn UserDriver> = Arc::new(SimUserDriver::new(
        app_ptr,
        rebind.window(),
        bounds.clone(),
        engine.clone(),
        runtime.handle().clone(),
        interaction_tx,
    ));
    let geometry: Box<dyn GeometryProvider> = Box::new(bounds.clone());
    let overlaid = overlay_windowed_caps(bundle.caps, geometry, engine.clone(), driver);

    // The window-settle hook the ComposedSut pumps before each check (mirror sim
    // `pump_cycle`: real wall-clock time for backend watchers on their own worker threads,
    // drain gpui, fire fake timers, promote staged bounds — no block_on, driver methods may
    // already be inside a tokio context).
    struct SendApp(*const TestApp);
    // SAFETY: the closure is only ever called on this gpui thread (inside `check_invariants`),
    // and `app` is pinned (never moved after `app_ptr` is taken; leaked at teardown) — the
    // same single-thread contract `SimUserDriver` relies on.
    unsafe impl Send for SendApp {}
    impl SendApp {
        // A `&self` accessor so the `move` closure captures the whole `SendApp` (Send) rather
        // than disjoint-capturing the raw-pointer field (2021 edition), which would be !Send.
        fn app(&self) -> &TestApp {
            unsafe { &*self.0 }
        }
    }
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

    // A dedicated runtime drives the apply/check leaf futures; the booted backend keeps
    // running on `runtime`'s worker threads (kept alive by the test's Arc). The gpui thread
    // is NOT runtime-entered, so `rt.block_on` inside the harness is legal here.
    let composed_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("composed runtime");

    // Assemble the windowed SUT (drives the initial page-root focus-align through the
    // overlaid caps, then wraps them via `from_parts`).
    let sut = windowed_composed_sut(overlaid, resolver, scaffold, composed_rt, settle);

    // ★ THE MILESTONE: the real StateMachineTest check runs the UNIFIED catalog green over
    // the initial rendered frame. Its internal windowed non-vacuity floor asserts
    // inv-frontend-bounds-rendered actually ran (a window is attached).
    ComposedSut::<WideE2E>::check_invariants(&sut, &oracle);

    eprintln!(
        "[compose_sut-windowed-3b] PASS - ComposedSut<WideE2E>::check_invariants ran the \
         unified composed catalog GREEN over the initial windowed frame (block/storage + \
         windowed families, one oracle, one SUT)"
    );

    // Teardown (see the other tests): release window entities, shut down, leak the !Send
    // app + the SUT (which transitively holds the session + the composed runtime) so no
    // Drop runs the gpui leak detector or shuts a runtime down in an async context.
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(sut);
}
