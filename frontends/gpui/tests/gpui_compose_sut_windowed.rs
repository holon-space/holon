//! ★ Round-5 windowed repoint — foundational milestone.
//!
//! ✅ VERIFIED GREEN (macOS, 2026-07-01) — 2 passed:
//!   `cargo test -p holon-gpui --features pbt --test gpui_compose_sut_windowed
//! -- --test-threads=1`
//!   1. window rendered 68 elements (63 non-degenerate) over the
//!      `compose_sut_windowed_base` session; base hosts `SutBackend` (13
//!      blocks); driver rung deferred.
//!   2. `overlay_windowed_caps` (runtime-exercised) built a CapMap with
//!      `SutLayout` (68 elems) + `SutBackend` (13 blocks) + the window's
//!      `SutDriver`/`SutBlockInteract` over a live window.
//! ⚠ MUST run with `--test-threads=1`: gpui `TestApp` is not parallel-safe
//! (thread-local platform state); two windowed tests in one binary SIGABRT if
//! run concurrently.
//!
//! Proves the claim the whole repoint rests on: a gpui window RENDERS a
//! [`compose_sut_windowed_base`] session (the window is a *pure renderer* over
//! a headless-booted `FrontendSession` + `ReactiveEngine`), and that
//! deferred-driver base already hosts the backend caps reading the booted
//! store. Together with the surfaced `session`/`reactive` handles, this shows
//! the windowed CapMap can be assembled by booting the headless composition and
//! attaching a window over its reactive engine — no separate booter, no new
//! id-reconcile.
//!
//! What is DEFERRED to a later increment (increment 3): the faithful windowed
//! gesture driver (`SimUserDriver`, which needs a live gpui `App` pointer +
//! `interaction_tx`) and the full `overlay_windowed_caps` + `StateMachineTest`
//! per-tick loop with a matched reference oracle. This milestone deliberately
//! reads `SutLayout` (through `window_layout`) + `SutBackend` (through the base
//! CapMap) directly, so it needs no reference matching.

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui::AssetSource;
use gpui::PlatformTextSystem;
use gpui::TestApp;
use holon_api::EntityUri;
use holon_api::Region;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::user_driver::UserDriver;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::composed::builder::compose_sut_windowed_base;
use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2EMachine;
use holon_integration_tests::pbt::fixtures::FixtureStep;
use holon_integration_tests::pbt::fixtures::replay_steps;
use holon_integration_tests::pbt::op_write_cap::IdResolver;
use holon_integration_tests::pbt::transitions::ClickBlock;
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_integration_tests::pbt::window_slice::builders::overlay_windowed_caps;
use holon_integration_tests::pbt::window_slice::builders::window_layout;
use holon_pbt_core::ComponentSet;
// Caps must be in scope to read them through the `CapMap` (capmap_adapter forwards).
use holon_pbt_core::capabilities::{SutBackend, SutBlockInteract, SutDriver, SutLayout};
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;
use pbt_harness::sim_windowed_replay::SimUserDriver;
// The wide windowed per-case boot/teardown helper (extracted to the shared harness so the
// 4b generated-sequence loop reuses ONE copy — see `pbt_harness/windowed_wide.rs`).
use pbt_harness::windowed_wide::with_windowed_wide_sut;

fn real_text_system() -> Arc<dyn PlatformTextSystem> {
    gpui_platform::current_platform(true).text_system()
}

/// Cross-runtime fixed-point settle (the proven `gpui_window_slice` pattern):
/// pump until the element count is stable and no `"loading"` placeholders
/// remain.
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
    //     `overlay_windowed_caps` INSERTS the window's driver caps as sole
    // providers.
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

    // (3) SutLayout over the window reads real, non-degenerate geometry — proving
    // the     window RENDERS compose_sut's session (the foundational Hard-B
    // claim).
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
        "[compose_sut-windowed] PASS — window rendered {} elements ({non_degenerate} \
         non-degenerate) over a compose_sut_windowed_base session; base hosts SutBackend ({} \
         booted blocks); driver deferred",
        via_capmap.len(),
        booted_blocks.len(),
    );

    // gpui teardown (mirror `gpui_window_slice.rs`): release the window entities,
    // shut the app down, then leak the `!Send` TestApp + the booted composition
    // so their Drops don't run the gpui leak detector / drop the session's
    // tokio runtime in async context. The process exits right after the test,
    // so the leak is inert.
    drop(_rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(composed);
}

#[test]
fn overlay_windowed_caps_composes_layout_backend_and_driver_over_a_live_window() {
    // Increment-3 sub-step 3a: runtime-exercise `overlay_windowed_caps` (until now
    // only compile-verified). Onto the DEFERRED-driver
    // `compose_sut_windowed_base` CapMap it must INSERT the window's
    // `SutLayout` geometry + the live `SimUserDriver`-backed gesture caps while
    // the base's `SutBackend` survives — the full windowed CapMap the
    // StateMachineTest runner (3b) will drive. Also de-risks the intricate
    // `SimUserDriver` construction over a compose_sut window.
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

    // ★ Exercise the pure-insert overlay at runtime. Its internal fail-loud assert
    // also confirms the base DEFERRED its driver (no SutDriver present) before
    // inserting.
    let geometry: Box<dyn GeometryProvider> = Box::new(bounds.clone());
    let frontend = composed
        .frontend
        .clone()
        .expect("full_headless → booted HeadlessFrontendComponent");
    let overlaid = overlay_windowed_caps(
        composed.caps,
        frontend,
        geometry,
        engine.clone(),
        driver,
        resolver.clone(),
    );

    // (1) The overlay INSERTED the window driver rung (absent in the deferred
    // base).
    assert!(
        overlaid.get::<dyn SutDriver>().is_some(),
        "overlay_windowed_caps must INSERT the window SutDriver",
    );
    assert!(
        overlaid.get::<dyn SutBlockInteract>().is_some(),
        "overlay_windowed_caps must INSERT the window SutBlockInteract gesture cap",
    );
    // (2) SutLayout reads real geometry through the overlaid CapMap (window renders
    // it).
    let elems = runtime.block_on(async { overlaid.rendered_elements().await });
    assert!(
        !elems.is_empty(),
        "overlaid CapMap's SutLayout returned no geometry",
    );
    // (3) The base's SutBackend survived the overlay (still reads the booted
    // store).
    let blocks = runtime.block_on(async { overlaid.block_raw_snapshot().await });
    assert!(
        !blocks.is_empty(),
        "overlaid CapMap lost the base SutBackend",
    );

    eprintln!(
        "[compose_sut-overlay] PASS — overlay_windowed_caps built a CapMap with SutLayout ({} \
         elems),          SutBackend ({} blocks), and the window's SutDriver + SutBlockInteract \
         over a live window",
        elems.len(),
        blocks.len(),
    );

    // gpui teardown (see the foundational test): release window entities, shut
    // down, then leak the `!Send` app + the overlaid caps (which transitively
    // hold the session) so no Drop runs the leak detector or drops the
    // session's runtime in async context.
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(overlaid);
}

#[test]
fn windowed_composed_sut_runs_full_catalog_green_on_the_initial_frame() {
    // ★ Increment 3b (sub-step i): the windowed StateMachineTest runner's
    // foundational check. Run the UNIFIED composed catalog through the real
    // `StateMachineTest::check_invariants` over the initial rendered frame —
    // block/storage families AND the windowed geometry/focus families, GREEN
    // against ONE `wide_e2e_ref()` oracle in a single SUT (the repoint's whole
    // point: one SUT, not E2ESut + a parallel windowed check).
    with_windowed_wide_sut(|sut, oracle| {
        ComposedSut::<WideE2E>::check_invariants(&sut, oracle);
        eprintln!(
            "[compose_sut-windowed-3b] PASS - ComposedSut<WideE2E>::check_invariants ran the \
             unified composed catalog GREEN over the initial windowed frame (block/storage + \
             windowed families, one oracle, one SUT)"
        );
        Some(sut)
    });
}

#[test]
fn windowed_composed_sut_drives_a_click_gesture_sequence_green() {
    // ★ Increment 3b (sub-step ii): drive a short HAND-BUILT gesture sequence
    // through the REAL `StateMachineTest::apply` path over the window. Each
    // `ClickBlock` focuses a text child via the window's `SimUserDriver`
    // (`SutBlockInteract` -> click -> `set_focus`, which mirrors engine focus —
    // the faithful windowed focus path, unlike the raw `SutFocusWrite` write).
    // Every tick re-checks the unified catalog. `ClickBlock` is non-minting, so the
    // per-tick id-reconcile is a no-op. Proves the apply -> window-gesture ->
    // settle -> check loop.
    with_windowed_wide_sut(|mut sut, oracle0| {
        // The initial frame must be green before we drive anything.
        ComposedSut::<WideE2E>::check_invariants(&sut, oracle0);

        // c1 / c2 are the wide working-tree text leaves (`block:c1` / `block:c2`, under
        // the seed page). Clicking a text child focuses it and opens its
        // editor.
        let steps = [
            E2ETransition::ClickBlock(ClickBlock {
                region: Region::Main,
                block_id: EntityUri::block("c1"),
            }),
            E2ETransition::ClickBlock(ClickBlock {
                region: Region::Main,
                block_id: EntityUri::block("c2"),
            }),
        ];

        let mut oracle = oracle0.clone();
        for (i, t) in steps.into_iter().enumerate() {
            assert!(
                <WideE2EMachine as ReferenceStateMachine>::preconditions(&oracle, &t),
                "step {i}: preconditions failed for {t:?} — the hand-built sequence encodes a \
                 stale assumption about the wide tree"
            );
            oracle = <WideE2EMachine as ReferenceStateMachine>::apply(oracle, &t);
            sut = ComposedSut::<WideE2E>::apply(sut, &oracle, t.clone());
            ComposedSut::<WideE2E>::check_invariants(&sut, &oracle);
            eprintln!("[compose_sut-windowed-3b-ii] step {i} ({t:?}) GREEN");
        }

        eprintln!(
            "[compose_sut-windowed-3b-ii] PASS - drove a 2-gesture ClickBlock sequence through \
             the windowed ComposedSut<WideE2E>; unified catalog GREEN each tick"
        );
        Some(sut)
    });
}

#[test]
fn windowed_composed_sut_replays_a_fixture_via_replay_steps_green() {
    // ★ Increment 3b (sub-step iii): the capture/gherkin BRIDGE over the windowed
    // SUT. Drive a fixture through `replay_steps` (the shared capture/.feature
    // replay driver) — the windowed `ComposedSut<WideE2E>` already impls
    // `FixtureAssertable`, so the SAME driver the headless composed keystone
    // uses for deterministic regression/gherkin replays now runs over a live
    // window. This is how captured windowed regressions + `.feature` files will
    // steer the ONE PBT windowed. The fixture is post-boot only (no `StartApp`
    // — the composed alphabet has none; the SUT is already booted by the
    // harness), matching composed-keystone captures.
    with_windowed_wide_sut(|sut, oracle| {
        let steps = vec![
            FixtureStep::Action(E2ETransition::ClickBlock(ClickBlock {
                region: Region::Main,
                block_id: EntityUri::block("c1"),
            })),
            FixtureStep::Action(E2ETransition::ClickBlock(ClickBlock {
                region: Region::Main,
                block_id: EntityUri::block("c2"),
            })),
        ];
        let sut = replay_steps::<WideE2EMachine, ComposedSut<WideE2E>>(
            "windowed-3b-iii",
            &steps,
            oracle.clone(),
            sut,
            |_| {},
            |_, _| {},
            None,
        );
        eprintln!(
            "[compose_sut-windowed-3b-iii] PASS - replay_steps drove a {}-step fixture through \
             the windowed ComposedSut<WideE2E> (FixtureAssertable bridge); catalog GREEN each tick",
            steps.len()
        );
        Some(sut)
    });
}

// ═══════════════════════════════════════════════════════════════════
// SPLIT-BOUNDS REGRESSION — windowed id-minting reconcile through the driver.
//
// A `SplitBlock` mints a FRESH uuid in the real SUT backend; the composed
// reconcile maps the oracle's synthetic `block::split-N` to that uuid in the
// shared `IdResolver`. Before the fix, `overlay_windowed_caps` built the
// windowed `DriverInputComponent` WITHOUT that resolver (`with_input`,
// `resolver: None`), so the bounds precheck resolved `block::split-N` to
// itself and false-failed `no registered bounds` — even though the row had
// rendered under its real uuid. This drives Split→ClickBlock on the minted
// block N times in one boot; each `ClickBlock` bounds-prechecks the freshly
// minted row. RED before the fix on iter 0 (identity resolve → wrong id);
// GREEN after (shared resolver → real uuid → registered bounds found).
// ═══════════════════════════════════════════════════════════════════
#[test]
fn windowed_split_then_clickblock_resolves_minted_id() {
    use std::collections::BTreeSet;

    use holon_integration_tests::pbt::transitions::SplitBlock;

    with_windowed_wide_sut(|mut sut, oracle0| {
        ComposedSut::<WideE2E>::check_invariants(&sut, oracle0);
        let mut oracle = oracle0.clone();
        let iters: usize = std::env::var("SPLIT_AMP_ITERS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8);
        // Chain: split `c1` first, then split each freshly-minted tail (it carries
        // the "c1" content, so it stays a splittable text leaf).
        let mut target = EntityUri::block("c1");
        let mut landed = 0usize;
        for i in 0..iters {
            let split = E2ETransition::SplitBlock(SplitBlock {
                block_id: target.clone(),
                position: 0,
            });
            if !<WideE2EMachine as ReferenceStateMachine>::preconditions(&oracle, &split) {
                eprintln!("[split-reg] iter {i}: SplitBlock precondition false on {target}, stop");
                break;
            }
            let before: BTreeSet<EntityUri> =
                oracle.domain.block_state.blocks.keys().cloned().collect();
            oracle = <WideE2EMachine as ReferenceStateMachine>::apply(oracle, &split);
            let after: BTreeSet<EntityUri> =
                oracle.domain.block_state.blocks.keys().cloned().collect();
            sut = ComposedSut::<WideE2E>::apply(sut, &oracle, split);
            ComposedSut::<WideE2E>::check_invariants(&sut, &oracle);
            let minted: Vec<EntityUri> = after.difference(&before).cloned().collect();
            assert_eq!(
                minted.len(),
                1,
                "iter {i}: expected 1 minted split id, got {minted:?}"
            );
            let new_id = minted[0].clone();
            // The regression probe: ClickBlock's FIRST act is `require_bounds` on the
            // minted (synthetic) id — the exact precheck that false-failed pre-fix.
            let click = E2ETransition::ClickBlock(ClickBlock {
                region: Region::Main,
                block_id: new_id.clone(),
            });
            assert!(
                <WideE2EMachine as ReferenceStateMachine>::preconditions(&oracle, &click),
                "iter {i}: ClickBlock precondition false for minted {new_id}"
            );
            oracle = <WideE2EMachine as ReferenceStateMachine>::apply(oracle, &click);
            sut = ComposedSut::<WideE2E>::apply(sut, &oracle, click);
            ComposedSut::<WideE2E>::check_invariants(&sut, &oracle);
            landed += 1;
            eprintln!("[split-reg] iter {i}: minted {new_id}, ClickBlock bounds precheck GREEN");
            target = new_id;
        }
        assert!(
            landed >= 1,
            "regression vacuous: no split→click iteration ran"
        );
        eprintln!(
            "[split-reg] PASS — {landed} split→ClickBlock iteration(s) resolved the minted id and found registered bounds"
        );
        Some(sut)
    });
}
