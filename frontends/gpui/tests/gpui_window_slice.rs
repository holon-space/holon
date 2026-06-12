//! E4 vertical slice: prove a composed `CapMap` hosts the windowed `SutLayout`
//! cap and reads **real** geometry through it — the first time the windowed cap
//! runs on the composition path rather than via `E2ESut`.
//!
//! Architecture under test (the E4 Send/`!Send` split):
//!   - The `!Send` gpui `TestApp` + frame-pump live here in the harness; we settle
//!     the window to a fixed point.
//!   - `GpuiWindowComponent` (in the `holon-integration-tests` lib) holds only the
//!     `Send` `BoundsRegistry` clone (as `Box<dyn GeometryProvider>`) and provides
//!     `SutLayout` on a `CapMap` like any other component.
//!   - We then read `SutLayout::rendered_elements` **through the `CapMap`** (the
//!     `#[capmap_adapter]`-generated `impl SutLayout for CapMap` forward) and assert
//!     it returns the same real, non-degenerate geometry the raw `BoundsRegistry`
//!     holds — i.e. the composed map faithfully hosts the windowed realization.
//!
//! **E4 increment 2 (this file):** the same booted window now also backs a
//! `SutViewModel` + `SutRenderer` (`window_wide` composes a
//! `GpuiFrontendEngineComponent` over the window's *own* frontend
//! `ReactiveEngine`), and `RefLayout` is registered on the ref `CapMap`. So the
//! windowed **registry** invariants — `inv-frontend-bounds-rendered` and the
//! `inv-displayed-text/{widget,viewmodel}` family — are selected and run via
//! `run_selected` over the real geometry, on the composition path.
//!
//! **E4 increment 3a (this file):** after the increment-2 run, the test grafts the
//! fixed shared `parent`/`c1`/`c2` tree under the Main panel's focus root
//! (`window_slice::seed::graft_displayed_text_tree`) and seeds the ref with the
//! SAME ids+content. **Both** windowed `inv-displayed-text` arms now *compare* those
//! grafted blocks (rather than skipping every unknown vault block) and reach `Ok`;
//! a paired planted `c1` divergence makes **both** `Fail`. That clean-pass/planted-
//! fail pair, at both layers, proves the windowed text oracle is non-vacuous on the
//! composition path.
//!
//! `/widget` reads the on-screen geometry (`SutLayout`); `/viewmodel` reads the
//! ViewModel tree, which the component resolves via the engine's recursive
//! `snapshot` (warm window watches) so it descends through the Main-panel
//! `live_block` into the grafted content — not the cold one-level `interpret_pure`
//! that left it empty.
//!
//! Defers (later E4 increments): the `StateMachineTest` windowed driver loop
//! (3b), and `SutDriver`→`window_focus` (increment 4).

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{AssetSource, PlatformTextSystem, TestApp};
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::BuilderServices;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::composed::composed_invariant_catalog;
use holon_integration_tests::pbt::window_slice::builders::{
    window_focus_wide, window_focus_wide_planted, window_layout, window_ref_caps,
    window_ref_caps_planted, window_ref_caps_seeded, window_wide,
};
use holon_integration_tests::pbt::window_slice::seed::graft_displayed_text_tree;
use holon_integration_tests::test_environment::TestEnvironment;
// `SutLayout` must be in scope to read geometry through the `CapMap`.
use holon_pbt_core::capabilities::{EngineFocus, SutLayout};
use holon_pbt_core::composition::run_selected;
use holon_pbt_core::invariant::InvariantResult;

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;
use holon_frontend::user_driver::UserDriver;
use pbt_harness::sim_windowed_replay::SimUserDriver;

fn real_text_system() -> Arc<dyn PlatformTextSystem> {
    gpui_platform::current_platform(true).text_system()
}

/// Cross-runtime fixed-point settle (same proven pattern as `test_platform_smoke`
/// / `test_platform_geometry_determinism`): pump until the element count is stable
/// and no `"loading"` placeholders remain. Panics if it never settles.
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
fn capmap_hosts_windowed_sutlayout_over_real_geometry() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = TestApp::with_text_system_and_assets(text_system, assets);

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let mut env = runtime
        .block_on(async { TestEnvironment::new(runtime.clone()) })
        .expect("test environment");
    runtime.block_on(async { env.start_app(true).await.expect("start_app") });

    let session = env.session_arc();
    let engine = env
        .reactive_engine
        .get()
        .cloned()
        .expect("reactive engine after start_app");
    let debug_services = env.debug_services().cloned().expect("debug services");

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
                Some(debug_services.clone()),
                "Holon-Windowed-Slice",
                cx,
            )
        })
        .expect("window opened");

    // Harness settle (the windowed realization's pump-to-fixed-point).
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    // The component holds only the Send geometry handle; build the composed CapMap.
    let geometry: Box<dyn GeometryProvider> = Box::new(bounds.clone());
    let capmap = window_layout(geometry);

    // Read SutLayout::rendered_elements THROUGH the CapMap (the capmap_adapter
    // forward), and the raw BoundsRegistry directly, and assert they agree.
    let via_capmap = runtime.block_on(async { capmap.rendered_elements().await });
    let raw = bounds.all_elements();

    eprintln!(
        "[windowed-slice] CapMap.rendered_elements()={} elements; raw BoundsRegistry={}",
        via_capmap.len(),
        raw.len()
    );

    // (1) Non-empty + non-degenerate real geometry, read through the CapMap.
    assert!(
        !via_capmap.is_empty(),
        "CapMap-hosted SutLayout returned no geometry"
    );
    let non_degenerate = via_capmap
        .iter()
        .filter(|e| e.width > 1.0 && e.height > 1.0)
        .count();
    assert!(
        non_degenerate >= 1,
        "CapMap-hosted SutLayout returned only degenerate geometry"
    );

    // (2) Faithful: the CapMap forward sees exactly the BoundsRegistry's elements.
    assert_eq!(
        via_capmap.len(),
        raw.len(),
        "CapMap-hosted SutLayout dropped/added elements vs the raw BoundsRegistry"
    );

    // (3) The conversion carried real widget data through (not all-empty).
    let with_widget_type = via_capmap
        .iter()
        .filter(|e| !e.widget_type.is_empty())
        .count();
    assert_eq!(
        with_widget_type,
        via_capmap.len(),
        "some RenderedElement lost its widget_type through the CapMap conversion"
    );

    eprintln!(
        "[windowed-slice] SutLayout OK — CapMap-hosted SutLayout read {} real \
         elements ({non_degenerate} non-degenerate) over a live TestPlatform window",
        via_capmap.len()
    );

    // ── E4 increment 2: the full windowed registry invariants via run_selected ──
    //
    // Compose the windowed SUT: the same window's geometry (`SutLayout`) AND its
    // own frontend `ReactiveEngine` (`SutViewModel` + `SutRenderer`). The ref is
    // a minimal honest oracle carrying `RefLayout`/`RefBlockTree`/`RefEditorMirror`.
    // `run_selected` then selects exactly the invariants this slice's caps satisfy
    // and runs them over the real geometry — the composition path, not `E2ESut`.
    let sut = window_wide(Box::new(bounds.clone()), engine.clone());
    let ref_caps = window_ref_caps();
    let report = runtime.block_on(run_selected(&composed_invariant_catalog(), &sut, &ref_caps));

    let ran = report.ran_ids();
    eprintln!("[windowed-slice] run_selected ran={ran:?}");
    eprintln!(
        "[windowed-slice] run_selected deselected={:?}",
        report.deselected.iter().map(|d| d.0).collect::<Vec<_>>()
    );

    // The headline: the windowed geometry invariant was SELECTED (its
    // `SutLayout + SutViewModel + RefLayout` needs are met) and RAN over real
    // geometry — the first registry invariant to do so on the composition path.
    assert!(
        ran.contains(&"inv-frontend-bounds-rendered"),
        "the windowed slice must select inv-frontend-bounds-rendered; ran={ran:?}",
    );
    // …and it ran its STRICT geometry checks to a verdict of `Ok` — NOT `Skipped`
    // (which would mean the frontend root was still loading / `frontend_root_vm`
    // returned `None`, leaving the proof vacuous). This is what makes "passes over
    // real geometry" meaningful.
    let bounds_result = report
        .ran
        .iter()
        .find(|(id, _)| id.0 == "inv-frontend-bounds-rendered")
        .map(|(_, r)| r)
        .expect("inv-frontend-bounds-rendered must be in the ran set");
    assert!(
        matches!(bounds_result, InvariantResult::Ok),
        "inv-frontend-bounds-rendered must reach Ok over the settled window (not \
         Skipped/Fail), got: {bounds_result:?}",
    );
    // The displayed-text family runs over the windowed slice too (`/widget` via
    // `SutLayout`, `/viewmodel` via `SutRenderer`).
    for id in ["inv-displayed-text/widget", "inv-displayed-text/viewmodel"] {
        assert!(
            ran.contains(&id),
            "the windowed slice must select {id}; ran={ran:?}",
        );
    }

    // Every selected invariant passes over the real, settled window geometry.
    assert!(
        report.failures().is_empty(),
        "windowed registry invariants must pass over real geometry: {:?}",
        report.failures(),
    );

    eprintln!(
        "[windowed-slice] PASS — {} registry invariant(s) (incl. \
         inv-frontend-bounds-rendered + inv-displayed-text) ran and passed over a \
         live TestPlatform window via run_selected",
        ran.len(),
    );

    // ── E4 increment 3a: graft a fixed-id tree → the displayed-text oracle BITES ──
    //
    // Until now the windowed `inv-displayed-text` ran but compared nothing: the
    // honest empty oracle knows none of the vault's random-UUID blocks, so every
    // text widget was skipped (unknown block ⇒ skip). 3a grafts the fixed shared
    // `parent`/`c1`/`c2` tree under the Main focus root and seeds the ref with the
    // SAME ids+content, so the rendered widgets resolve to ref-known blocks and the
    // comparison runs for real. A clean pass + a planted-divergence FAIL together
    // prove the windowed text oracle is non-vacuous.
    runtime
        .block_on(graft_displayed_text_tree(&env))
        .expect("graft fixed-id tree under Main focus root");
    // Re-settle: the new blocks must propagate (Turso → CDC → engine → paint) and
    // the window must repaint them before the invariants read geometry/VM.
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    // Both displayed-text arms compare the grafted content: `/widget` reads the
    // on-screen geometry (`SutLayout`), and `/viewmodel` reads the ViewModel tree —
    // which the component resolves via the engine's RECURSIVE `snapshot` (warm
    // window watches), so it descends through the Main-panel `live_block` into the
    // grafted blocks. First prove the graft is genuinely on-screen so the passes
    // below are non-vacuous.
    let geo = runtime.block_on(async { sut.rendered_elements().await });
    for (id, want) in [
        ("block:c1", "c1"),
        ("block:parent", "parent"),
        ("block:c2", "c2"),
    ] {
        let shown = geo.iter().any(|e| {
            e.entity_id.as_ref().map(|u| u.as_str()) == Some(id)
                && e.displayed_text.as_deref() == Some(want)
        });
        assert!(
            shown,
            "grafted block {id} must render on-screen with text {want:?} (B1 graft \
             under the Main focus root) — else the oracle below is vacuous",
        );
    }

    let dt_arms = ["inv-displayed-text/widget", "inv-displayed-text/viewmodel"];
    let result_of =
        |report: &holon_pbt_core::composition::RunReport, id: &str| -> InvariantResult {
            report
                .ran
                .iter()
                .find(|(rid, _)| rid.0 == id)
                .map(|(_, r)| r.clone())
                .unwrap_or_else(|| panic!("{id} not in ran set; ran={:?}", report.ran_ids()))
        };

    // Clean run: the seeded ref knows parent/c1/c2 with matching content, so BOTH
    // arms COMPARE them (not skip) and must reach `Ok`.
    let seeded = window_ref_caps_seeded();
    let seeded_report =
        runtime.block_on(run_selected(&composed_invariant_catalog(), &sut, &seeded));
    for id in dt_arms {
        assert!(
            matches!(result_of(&seeded_report, id), InvariantResult::Ok),
            "{id} must reach Ok over the grafted+seeded window (NOT Skipped — that \
             would mean the grafted blocks didn't reach this layer), got: {:?}",
            result_of(&seeded_report, id),
        );
    }
    assert!(
        seeded_report.failures().is_empty(),
        "seeded windowed run must pass: {:?}",
        seeded_report.failures(),
    );
    eprintln!("[3a] seeded run: inv-displayed-text/{{widget,viewmodel}} both reached Ok");

    // Planted run: same grafted window, but the ref claims `c1 = c1-WRONG`. BOTH
    // arms MUST now fail on c1 — the negative control proving each layer genuinely
    // compares the grafted content (not a vacuous skip/pass).
    let planted = window_ref_caps_planted();
    let planted_report =
        runtime.block_on(run_selected(&composed_invariant_catalog(), &sut, &planted));
    let planted_failures = planted_report.failures();
    let failed_ids: Vec<&str> = planted_failures.iter().map(|(id, _)| *id).collect();
    for id in dt_arms {
        assert!(
            failed_ids.contains(&id),
            "planted c1 divergence must make {id} FAIL (the windowed oracle bites); \
             instead failures={planted_failures:?}",
        );
    }
    eprintln!(
        "[3a] planted run: inv-displayed-text/{{widget,viewmodel}} both FAILED on the \
         c1 divergence — windowed oracle bites at both layers"
    );

    // ── E4 increment 3b: drive a focus, run the windowed focus invariant ──────────
    //
    // On boot no block is in edit mode, so no `editable_text` mounts and both focus
    // authorities are unfocused — `inv-window-focus-matches-engine-focus` would
    // Skip ("both authorities unfocused"), which is vacuous. Drive a REAL click on
    // the grafted `block:c1` (through the windowed `SimUserDriver`, the same driver
    // the windowed PBT loop uses): that mounts c1's `editable_text` and moves engine
    // focus to it. Then run the focus invariant over the composed `CapMap`:
    //   - clean (`window_focus_wide`, engine read live)  → engine == window == c1 → Ok
    //   - planted (`window_focus_wide_planted`, engine FORCED to c2) → engine c2 vs
    //     window c1 → Fail (steal-back / zombie editor, ADR 0010).
    // That clean/planted pair is the focus-axis analogue of 3a's displayed-text pair
    // — it proves the windowed focus oracle bites on the composition path, not via
    // `E2ESut`.
    let c1 = holon_api::EntityUri::block("c1");
    let interaction_tx = debug_services
        .interaction_tx
        .get()
        .expect("interaction_tx set by the window interaction pump")
        .clone();
    let app_ptr: *const TestApp = &app;
    let driver = SimUserDriver::new(
        app_ptr,
        _rebind.window(),
        bounds.clone(),
        engine.clone(),
        runtime.handle().clone(),
        interaction_tx,
    );
    runtime
        .block_on(async { driver.click_entity(&c1, "main").await })
        .expect("click block:c1 to focus it (mount its editable_text)");
    // Settle the editor mount + the spawned window-focus binding to a fixed point.
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    // Prove the click actually focused c1's editor on-screen, so the Ok below is
    // non-vacuous: c1 must be window-focused AND its editable_text mounted.
    let geo_focus = runtime.block_on(async { sut.rendered_elements().await });
    let window_focused: Vec<String> = geo_focus
        .iter()
        .filter(|e| e.focused == Some(true))
        .filter_map(|e| e.entity_id.as_ref().map(|u| u.as_str().to_string()))
        .collect();
    let c1_editor_mounted = geo_focus.iter().any(|e| {
        e.widget_type == "editable_text"
            && e.entity_id.as_ref().map(|u| u.as_str()) == Some("block:c1")
    });
    let engine_focus = engine.focused_block();
    eprintln!(
        "[3b] after click block:c1 — engine.focused_block()={engine_focus:?}; \
         window-focused={window_focused:?}; c1 editable_text mounted={c1_editor_mounted}"
    );
    assert!(
        window_focused.iter().any(|id| id == "block:c1") && c1_editor_mounted,
        "click must focus block:c1's editable_text on-screen (else the Ok below is \
         vacuous); window-focused={window_focused:?}, c1_editor_mounted={c1_editor_mounted}",
    );

    let wf_id = "inv-window-focus-matches-engine-focus";

    // Clean: the composed CapMap reads the live engine focus (= c1) and the window
    // focus (= c1) — they agree, so the focus invariant reaches Ok over real geometry.
    let focus_sut = window_focus_wide(Box::new(bounds.clone()), engine.clone());
    let focus_report = runtime.block_on(run_selected(
        &composed_invariant_catalog(),
        &focus_sut,
        &seeded,
    ));
    assert!(
        focus_report.ran_ids().contains(&wf_id),
        "window_focus must be SELECTED by the full windowed slice (SutDriver+SutLayout); \
         ran={:?}",
        focus_report.ran_ids(),
    );
    assert!(
        matches!(result_of(&focus_report, wf_id), InvariantResult::Ok),
        "{wf_id} must reach Ok when engine focus (c1) matches window focus (c1), got: {:?}",
        result_of(&focus_report, wf_id),
    );
    eprintln!("[3b] clean run: {wf_id} reached Ok (engine focus == window focus == block:c1)");

    // Planted: same window (c1 still window-focused), but the SutDriver's
    // engine_focused_block is FORCED to report block:c2. Engine claims c2 while the
    // window shows c1 → the invariant must Fail (the steal-back it exists to catch).
    let planted_focus_sut = window_focus_wide_planted(
        Box::new(bounds.clone()),
        engine.clone(),
        EngineFocus::Focused(holon_api::EntityUri::block("c2")),
    );
    let planted_focus_report = runtime.block_on(run_selected(
        &composed_invariant_catalog(),
        &planted_focus_sut,
        &seeded,
    ));
    let planted_focus_failures: Vec<&str> = planted_focus_report
        .failures()
        .iter()
        .map(|(id, _)| *id)
        .collect();
    assert!(
        planted_focus_failures.contains(&wf_id),
        "planted engine/window focus divergence (engine forced to c2, window shows c1) \
         must make {wf_id} FAIL; instead failures={planted_focus_failures:?}",
    );
    eprintln!(
        "[3b] planted run: {wf_id} FAILED on the engine(c2)/window(c1) divergence — \
         windowed focus oracle bites on the composition path"
    );

    // Leak the !Send TestApp (gpui leak detector); process exits after the test.
    drop(_rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(env);
}
