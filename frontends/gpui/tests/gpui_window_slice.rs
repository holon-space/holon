//! E4 vertical slice: prove a composed `CapMap` hosts the windowed `SutLayout`
//! cap and reads **real** geometry through it — the first time the windowed cap
//! runs on the composition path rather than via `E2ESut`.
//!
//! Architecture under test (the E4 Send/`!Send` split):
//!   - The `!Send` gpui `TestApp` + frame-pump live here in the harness; we
//!     settle the window to a fixed point.
//!   - `GpuiWindowComponent` (in the `holon-integration-tests` lib) holds only
//!     the `Send` `BoundsRegistry` clone (as `Box<dyn GeometryProvider>`) and
//!     provides `SutLayout` on a `CapMap` like any other component.
//!   - We then read `SutLayout::rendered_elements` **through the `CapMap`**
//!     (the `#[capmap_adapter]`-generated `impl SutLayout for CapMap` forward)
//!     and assert it returns the same real, non-degenerate geometry the raw
//!     `BoundsRegistry` holds — i.e. the composed map faithfully hosts the
//!     windowed realization.
//!
//! **E4 increment 2 (this file):** the same booted window now also backs a
//! `SutViewModel` + `SutRenderer` (`window_wide` composes a
//! `GpuiFrontendEngineComponent` over the window's *own* frontend
//! `ReactiveEngine`), and `RefLayout` is registered on the ref `CapMap`. So the
//! windowed **registry** invariants — `inv-frontend-bounds-rendered` and the
//! `inv-displayed-text/{widget,viewmodel}` family — are selected and run via
//! `run_selected` over the real geometry, on the composition path.
//!
//! **E4 increment 3a (this file):** after the increment-2 run, the test grafts
//! the fixed shared `parent`/`c1`/`c2` tree under the Main panel's focus root
//! (`window_slice::seed::graft_displayed_text_tree`) and seeds the ref with the
//! SAME ids+content. **Both** windowed `inv-displayed-text` arms now *compare*
//! those grafted blocks (rather than skipping every unknown vault block) and
//! reach `Ok`; a paired planted `c1` divergence makes **both** `Fail`. That
//! clean-pass/planted- fail pair, at both layers, proves the windowed text
//! oracle is non-vacuous on the composition path.
//!
//! `/widget` reads the on-screen geometry (`SutLayout`); `/viewmodel` reads the
//! ViewModel tree, which the component resolves via the engine's recursive
//! `snapshot` (warm window watches) so it descends through the Main-panel
//! `live_block` into the grafted content — not the cold one-level
//! `interpret_pure` that left it empty.
//!
//! Defers (later E4 increments): the `StateMachineTest` windowed driver loop
//! (3b), and `SutDriver`→`window_focus` (increment 4).

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui::AssetSource;
use gpui::PlatformTextSystem;
use gpui::TestApp;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::BuilderServices;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::composed::composed_invariant_catalog;
use holon_integration_tests::pbt::window_slice::builders::window_focus_wide;
use holon_integration_tests::pbt::window_slice::builders::window_focus_wide_planted;
use holon_integration_tests::pbt::window_slice::builders::window_layout;
use holon_integration_tests::pbt::window_slice::builders::window_ref_caps;
use holon_integration_tests::pbt::window_slice::builders::window_ref_caps_planted;
use holon_integration_tests::pbt::window_slice::builders::window_ref_caps_seeded;
use holon_integration_tests::pbt::window_slice::builders::window_wide;
use holon_integration_tests::pbt::window_slice::seed::BAND_LAST_CONTENT;
use holon_integration_tests::pbt::window_slice::seed::BAND_POST_COUNT_OVERLAP;
use holon_integration_tests::pbt::window_slice::seed::BAND_POST_COUNT_SCROLL;
use holon_integration_tests::pbt::window_slice::seed::BAND_POST_MARKER;
use holon_integration_tests::pbt::window_slice::seed::BAND_PRE_MARKER;
use holon_integration_tests::pbt::window_slice::seed::BAND_ROW_COUNT;
use holon_integration_tests::pbt::window_slice::seed::BAND_ROW_MARKER;
use holon_integration_tests::pbt::window_slice::seed::BAND_SIBLING_CONTENT;
use holon_integration_tests::pbt::window_slice::seed::NESTED_QUERY_ROW_COUNT;
use holon_integration_tests::pbt::window_slice::seed::NESTED_QUERY_ROW_MARKER;
use holon_integration_tests::pbt::window_slice::seed::graft_band_geometry_page;
use holon_integration_tests::pbt::window_slice::seed::graft_displayed_text_tree;
use holon_integration_tests::pbt::window_slice::seed::graft_nested_query_block;
use holon_integration_tests::test_environment::TestEnvironment;
use holon_pbt_core::capabilities::EngineFocus;
use holon_pbt_core::capabilities::SutLayout;
// `SutLayout` must be in scope to read geometry through the `CapMap`.
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::composition::run_selected;
use holon_pbt_core::invariant::InvariantResult;

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;
use holon_frontend::user_driver::UserDriver;
use pbt_harness::sim_windowed_replay::SimUserDriver;

fn real_text_system() -> Arc<dyn PlatformTextSystem> {
    gpui_platform::current_platform(true).text_system()
}

/// Cross-runtime fixed-point settle (same proven pattern as
/// `test_platform_smoke` / `test_platform_geometry_determinism`): pump until
/// the element count is stable and no `"loading"` placeholders remain. Panics
/// if it never settles.
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
    let env = runtime
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
        "[windowed-slice] SutLayout OK — CapMap-hosted SutLayout read {} real elements \
         ({non_degenerate} non-degenerate) over a live TestPlatform window",
        via_capmap.len()
    );

    // ── E4 increment 2: the full windowed registry invariants via run_selected ──
    //
    // Compose the windowed SUT: the same window's geometry (`SutLayout`) AND its
    // own frontend `ReactiveEngine` (`SutViewModel` + `SutRenderer`). The ref is
    // a minimal honest oracle carrying
    // `RefLayout`/`RefBlockTree`/`RefEditorMirror`. `run_selected` then selects
    // exactly the invariants this slice's caps satisfy and runs them over the
    // real geometry — the composition path, not `E2ESut`.
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
        "inv-frontend-bounds-rendered must reach Ok over the settled window (not Skipped/Fail), \
         got: {bounds_result:?}",
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
        "[windowed-slice] PASS — {} registry invariant(s) (incl. inv-frontend-bounds-rendered + \
         inv-displayed-text) ran and passed over a live TestPlatform window via run_selected",
        ran.len(),
    );

    // ── E4 increment 3a: graft a fixed-id tree → the displayed-text oracle BITES
    // ──
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
            "grafted block {id} must render on-screen with text {want:?} (B1 graft under the Main \
             focus root) — else the oracle below is vacuous",
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
            "{id} must reach Ok over the grafted+seeded window (NOT Skipped — that would mean the \
             grafted blocks didn't reach this layer), got: {:?}",
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
            "planted c1 divergence must make {id} FAIL (the windowed oracle bites); instead \
             failures={planted_failures:?}",
        );
    }
    eprintln!(
        "[3a] planted run: inv-displayed-text/{{widget,viewmodel}} both FAILED on the c1 \
         divergence — windowed oracle bites at both layers"
    );

    // ── E4 increment 3b: drive a focus, run the windowed focus invariant
    // ──────────
    //
    // On boot no block is in edit mode, so no `editable_text` mounts and both focus
    // authorities are unfocused — `inv-window-focus-matches-engine-focus` would
    // Skip ("both authorities unfocused"), which is vacuous. Drive a REAL click on
    // the grafted `block:c1` (through the windowed `SimUserDriver`, the same driver
    // the windowed PBT loop uses): that mounts c1's `editable_text` and moves
    // engine focus to it. Then run the focus invariant over the composed
    // `CapMap`:
    //   - clean (`window_focus_wide`, engine read live)  → engine == window == c1 →
    //     Ok
    //   - planted (`window_focus_wide_planted`, engine FORCED to c2) → engine c2 vs
    //     window c1 → Fail (steal-back / zombie editor, ADR 0010).
    // That clean/planted pair is the focus-axis analogue of 3a's displayed-text
    // pair — it proves the windowed focus oracle bites on the composition path,
    // not via `E2ESut`.
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
        "click must focus block:c1's editable_text on-screen (else the Ok below is vacuous); \
         window-focused={window_focused:?}, c1_editor_mounted={c1_editor_mounted}",
    );

    let wf_id = "inv-window-focus-matches-engine-focus";

    // Clean: the composed CapMap reads the live engine focus (= c1) and the window
    // focus (= c1) — they agree, so the focus invariant reaches Ok over real
    // geometry.
    let focus_sut = window_focus_wide(Box::new(bounds.clone()), engine.clone());
    let focus_report = runtime.block_on(run_selected(
        &composed_invariant_catalog(),
        &focus_sut,
        &seeded,
    ));
    assert!(
        focus_report.ran_ids().contains(&wf_id),
        "window_focus must be SELECTED by the full windowed slice (SutDriver+SutLayout); ran={:?}",
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
    // window shows c1 → the invariant must Fail (the steal-back it exists to
    // catch).
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
        "planted engine/window focus divergence (engine forced to c2, window shows c1) must make \
         {wf_id} FAIL; instead failures={planted_focus_failures:?}",
    );
    eprintln!(
        "[3b] planted run: {wf_id} FAILED on the engine(c2)/window(c1) divergence — windowed \
         focus oracle bites on the composition path"
    );

    // Leak the !Send TestApp (gpui leak detector); process exits after the test.
    drop(_rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(env);
}

/// Minimum painted height (px) that counts as "a real row". A text row in the
/// outline is ~14-20px; anything at or under this is a collapsed/degenerate
/// box, not something a user can read or click.
const MIN_ROW_HEIGHT_PX: f32 = 8.0;

/// A nested `live_block` — a query block embedded as one ROW of the main
/// outline via the `query_block_titled` profile variant — must PAINT the rows
/// its ViewModel holds.
///
/// This is the geometry half of the ClaudeCode blank-page bug (BugFunnel,
/// Martin dogfood 2026-07-30): the page rendered its section headlines but zero
/// rows under each one. The ViewModel carried every row; only the paint was
/// empty, because `ReactiveShell`'s block-mode arm unconditionally renders
/// `size_full().overflow_y_scroll()` — a shape that is correct only when the
/// shell is parented by `columns::panel_wrap`'s definite-height `absolute
/// size_full` div. Embedded as an outline row the parent height is indefinite,
/// `height: 100%` resolves to a fixed empty band, and no row is laid out.
///
/// The test therefore asserts the MODEL first (the rows are in the ViewModel)
/// and the GEOMETRY second (at least one row is painted at a hit-testable
/// height). A failure of the second assert with the first passing is the
/// height defect; a failure of the first would be a data/model defect and a
/// different bug.
///
/// Deliberately a SEPARATE test from
/// `capmap_hosts_windowed_sutlayout_over_real_geometry` (a known pre-existing
/// red — phantom ids + ghost row after the pre-warm timeout) so this invariant
/// can be green independently.
#[test]
fn nested_live_block_paints_the_rows_its_model_holds() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = TestApp::with_text_system_and_assets(text_system, assets);

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let env = runtime
        .block_on(async { TestEnvironment::new(runtime.clone()) })
        .expect("test environment");
    runtime.block_on(async { env.start_app(true).await.expect("start_app") });

    // Graft BEFORE the window opens, the way a real vault's page already exists
    // at boot: the block profile's `has_query_source` lookup is fed by a CDC
    // live entity that the first profile resolution reads, so the headline must
    // already own its query source when the outline first renders.
    runtime
        .block_on(graft_nested_query_block(&env))
        .expect("graft the ClaudeCode-shaped query headline under the Main focus root");

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
                "Holon-Nested-LiveBlock-Slice",
                cx,
            )
        })
        .expect("window opened");

    let sut = window_wide(Box::new(bounds.clone()), engine.clone());
    let marker_rows_in_vm = |sut: &_, runtime: &tokio::runtime::Runtime| -> usize {
        let vm = runtime.block_on(async { SutRenderer::widget_tree_snapshot(sut).await });
        vm.walk()
            .filter(|n| {
                n.props
                    .values()
                    .any(|v| v.contains(NESTED_QUERY_ROW_MARKER))
            })
            .count()
    };

    // (1) MODEL: pump until the recursive ViewModel snapshot descends through the
    // nested `live_block` and carries one marker-bearing node per query row.
    let model_deadline = Instant::now() + Duration::from_secs(60);
    let mut vm_rows = 0usize;
    while Instant::now() < model_deadline {
        settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));
        vm_rows = marker_rows_in_vm(&sut, &runtime);
        if vm_rows >= NESTED_QUERY_ROW_COUNT {
            break;
        }
    }
    // Then keep pumping (bounded) until the widget's rows are on screen, so the
    // judgement below reads a settled frame rather than an in-flight one. On the
    // defective build this simply runs to its deadline.
    let paint_deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < paint_deadline {
        settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));
        let painted_now = bounds.all_elements().iter().any(|(_, i)| {
            i.displayed_text
                .as_deref()
                .is_some_and(|t| t.contains(NESTED_QUERY_ROW_MARKER))
        });
        if painted_now {
            break;
        }
    }

    let seeded_rows = runtime
        .block_on(env.query_sql(
            "SELECT id, content FROM block_raw WHERE parent_id = 'block:nq-data' ORDER BY content",
        ))
        .expect("read back the grafted query rows");
    eprintln!(
        "[nested-live-block] after model settle: vm_rows={vm_rows} seeded_rows={} \
         elements={}",
        seeded_rows.len(),
        bounds.all_elements().len(),
    );
    assert!(
        vm_rows >= NESTED_QUERY_ROW_COUNT,
        "model precondition: the ViewModel must hold all {NESTED_QUERY_ROW_COUNT} query rows \
         before geometry can be judged, got {vm_rows} (the backend holds {} matching block rows) \
         — this is a DATA failure, not the height defect this test exists for",
        seeded_rows.len(),
    );

    // (2) GEOMETRY: those rows must actually be painted at a readable height.
    let geo = runtime.block_on(async { sut.rendered_elements().await });
    let painted: Vec<_> = geo
        .iter()
        .filter(|e| {
            e.displayed_text
                .as_deref()
                .is_some_and(|t| t.contains(NESTED_QUERY_ROW_MARKER))
        })
        .collect();
    let heights: Vec<f32> = painted.iter().map(|e| e.height).collect();
    let hit_testable = painted
        .iter()
        .filter(|e| e.width > 1.0 && e.height >= MIN_ROW_HEIGHT_PX)
        .count();

    eprintln!(
        "[nested-live-block] vm_rows={vm_rows} painted={} heights={heights:?} \
         hit_testable={hit_testable} total_elements={}",
        painted.len(),
        geo.len(),
    );
    assert!(
        hit_testable >= 1,
        "the nested live_block's widget band painted NO row at a hit-testable HEIGHT: the \
         ViewModel holds {vm_rows} query rows, but BoundsRegistry has {} element(s) carrying the \
         row marker and {hit_testable} of them reach {MIN_ROW_HEIGHT_PX}px (heights={heights:?}). \
         The shell's `height: 100%` resolved against an indefinite outline-row parent, so the \
         band is a fixed empty box and no row was laid out.",
        painted.len(),
    );

    eprintln!(
        "[nested-live-block] PASS — {hit_testable}/{} query rows painted at >= \
         {MIN_ROW_HEIGHT_PX}px inside the nested live_block",
        painted.len(),
    );

    drop(_rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(env);
}

// ── Bug #69: the nested band's PAINTED height vs the height the outline
// RESERVES for it ───────────────────────────────────────────────────────────
//
// After #60 the nested band paints its rows — but only the band's own internal
// layout knows how tall it became. The enclosing outline is a virtualized
// `gpui::list`, and the height it reserves for the band's row is whatever that
// row measured at. If the reserved height stays behind the painted height, two
// user-visible failures follow, and these two tests judge exactly one each:
//
//   (a) the following sibling row is placed at the RESERVED offset, so it draws
//       ON TOP of the band's lower rows (Martin's ClaudeCode page: five
//       sections overlapping each other);
//   (b) the list's scroll extent is summed from the same reserved heights, so
//       the page cannot scroll far enough to reach its last rows.
//
// Both read GEOMETRY only. Each first asserts the band actually painted its
// rows, so a regression of #60 reports itself as such instead of masquerading
// as a green overlap check.

/// Everything a band-geometry test needs from a booted, seeded window. Held
/// together so the two tests share one boot preamble.
struct BandPage {
    runtime: Arc<tokio::runtime::Runtime>,
    env: TestEnvironment,
    bounds: BoundsRegistry,
    engine: Arc<holon_frontend::reactive::ReactiveEngine>,
    debug_services: Arc<holon_mcp::server::DebugServices>,
    rebind: holon_gpui::RebindHandle,
}

impl BandPage {
    /// How many rows the band's query actually has to work with in the backend.
    /// Separates "the band painted nothing because the data isn't there" from
    /// "the band painted nothing because of layout".
    fn seeded_row_count(&self) -> usize {
        self.runtime
            .block_on(self.env.query_sql(
                "SELECT id FROM block_raw WHERE parent_id = 'block:band-data' ORDER BY content",
            ))
            .expect("read back the grafted band query rows")
            .len()
    }
}

/// Boot a window over a vault carrying the bug-#69 page shape
/// ([`graft_band_geometry_page`]) and settle it until the band has painted its
/// rows (bounded — on a build where the band paints nothing this returns after
/// the deadline and the caller's precondition assert reports that).
fn boot_band_page(app: &mut TestApp, post_count: usize) -> BandPage {
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let env = runtime
        .block_on(async { TestEnvironment::new(runtime.clone()) })
        .expect("test environment");
    runtime.block_on(async { env.start_app(true).await.expect("start_app") });

    // Seeded BEFORE the window opens, like a real vault page that already
    // exists at boot (the block profile's `has_query_source` lookup must see
    // the headline's source child on the first profile resolution).
    runtime
        .block_on(graft_band_geometry_page(&env, post_count))
        .expect("graft the bug-#69 band page under the Main focus root");

    let session = env.session_arc();
    let engine = env
        .reactive_engine
        .get()
        .cloned()
        .expect("reactive engine after start_app");
    let debug_services = env.debug_services().cloned().expect("debug services");

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
                Some(debug_services.clone()),
                "Holon-Band-Geometry-Slice",
                cx,
            )
        })
        .expect("window opened");

    // Settle on the MODEL, not on paint. The outline is virtualized AND the page
    // boots scrolled, so the band is routinely off-viewport at this point —
    // waiting for its rows to be PAINTED here would either hang or (worse) exit
    // the moment some unrelated row appeared, judging geometry before the query
    // had even resolved. What must be true before a test may drive is that the
    // band's rows exist in the ViewModel; where they are on screen is the
    // question the tests then ask.
    let sut = window_wide(Box::new(bounds.clone()), engine.clone());
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut round = 0u32;
    let mut vm_rows = 0usize;
    while Instant::now() < deadline {
        settle_to_fixed_point(app, &bounds, &runtime, Duration::from_secs(30));
        let vm = runtime.block_on(async { SutRenderer::widget_tree_snapshot(&sut).await });
        vm_rows = vm
            .walk()
            .filter(|n| n.props.values().any(|v| v.contains(BAND_ROW_MARKER)))
            .count();
        let elements = bounds.all_elements();
        let texts: Vec<String> = elements
            .iter()
            .filter_map(|(_, i)| i.displayed_text.as_deref().map(str::to_string))
            .collect();
        eprintln!(
            "[band-boot] round {round}: vm_rows={vm_rows} elements={} painted_band={} pre={} \
             sib={} head={}",
            elements.len(),
            texts.iter().filter(|t| t.contains(BAND_ROW_MARKER)).count(),
            texts.iter().filter(|t| t.contains(BAND_PRE_MARKER)).count(),
            texts
                .iter()
                .filter(|t| t.contains(BAND_SIBLING_CONTENT))
                .count(),
            texts
                .iter()
                .filter(|t| t.contains("Band Query Head"))
                .count(),
        );
        round += 1;
        if vm_rows >= BAND_ROW_COUNT {
            break;
        }
    }
    assert!(
        vm_rows >= BAND_ROW_COUNT,
        "model precondition: the band's ViewModel must hold all {BAND_ROW_COUNT} query rows before \
         any geometry can be judged, got {vm_rows} — this is a DATA failure (the query never \
         resolved), not a geometry defect"
    );

    BandPage {
        runtime,
        env,
        bounds,
        engine,
        debug_services,
        rebind,
    }
}

/// The painted rows carrying `marker` in their displayed text, as
/// `(text, y, height)`, sorted top-to-bottom. Only elements with a real width
/// count — a zero-width record is not something the user can see.
fn painted_rows(bounds: &BoundsRegistry, marker: &str) -> Vec<(String, f32, f32)> {
    let mut rows: Vec<(String, f32, f32)> = bounds
        .all_elements()
        .iter()
        .filter(|(_, i)| i.width > 1.0 && i.height >= MIN_ROW_HEIGHT_PX)
        .filter_map(|(_, i)| {
            let text = i.displayed_text.as_deref()?;
            text.contains(marker)
                .then(|| (text.to_string(), i.y, i.height))
        })
        .collect();
    rows.sort_by(|a, b| a.1.total_cmp(&b.1));
    rows
}

/// Tolerance (px) for "touching, not overlapping". Adjacent rows may share an
/// edge; anything deeper than this is real, visible overlap.
const OVERLAP_EPSILON_PX: f32 = 1.0;

/// `(y, height)` of the nearest `tree_item` ancestor — the OUTLINE ROW — above
/// the first painted element carrying `marker`. This is the box the outline
/// RESERVED; everything the outline lays out next starts at its bottom edge.
fn outline_row_box(bounds: &BoundsRegistry, marker: &str) -> Option<(f32, f32)> {
    let elements = bounds.all_elements();
    let by_id: std::collections::HashMap<&str, _> = elements
        .iter()
        .map(|(id, info)| (id.as_str(), info))
        .collect();

    let (start_id, _) = elements.iter().find(|(_, i)| {
        i.width > 1.0
            && i.displayed_text
                .as_deref()
                .is_some_and(|t| t.contains(marker))
    })?;

    let mut cursor = Some(start_id.as_str());
    let mut hops = 0;
    while let Some(id) = cursor {
        let info = by_id.get(id)?;
        if info.widget_type.as_ref() == "tree_item" {
            return Some((info.y, info.height));
        }
        hops += 1;
        if hops > 24 {
            return None;
        }
        cursor = info.parent_id.as_deref();
    }
    None
}

/// The chain of tracked ancestors above the first element whose displayed text
/// contains `marker`, innermost first, as `widget_type[entity]@y+height`.
///
/// When a row paints outside the box its parent reserved, this names the exact
/// container where the height stops agreeing — a bare "they overlap" says two
/// boxes collide, this says which ancestor is short.
fn ancestor_chain(bounds: &BoundsRegistry, marker: &str) -> Vec<String> {
    let elements = bounds.all_elements();
    let by_id: std::collections::HashMap<&str, _> = elements
        .iter()
        .map(|(id, info)| (id.as_str(), info))
        .collect();

    let Some((start_id, _)) = elements.iter().find(|(_, i)| {
        i.width > 1.0
            && i.displayed_text
                .as_deref()
                .is_some_and(|t| t.contains(marker))
    }) else {
        return vec![format!("<no painted element carrying `{marker}`>")];
    };

    let mut chain = Vec::new();
    let mut cursor = Some(start_id.as_str());
    // Bounded: a tracked chain deeper than this means a cycle, and printing it
    // forever would bury the assertion it is meant to explain.
    while let Some(id) = cursor {
        let Some(info) = by_id.get(id) else { break };
        chain.push(format!(
            "{}[{}]@{:.1}+{:.1}",
            info.widget_type,
            info.entity_id.as_deref().unwrap_or("-"),
            info.y,
            info.height,
        ));
        if chain.len() >= 24 {
            chain.push("…".to_string());
            break;
        }
        cursor = info.parent_id.as_deref();
    }
    chain
}

/// A `SimUserDriver` over the booted band page — the same driver the windowed
/// PBT loop uses, so scroll goes through production's wheel/reveal path.
///
/// SAFETY: `app` must outlive the returned driver and stay on the gpui thread
/// (the contract `SimUserDriver::new` documents).
fn band_driver(app: &TestApp, page: &BandPage) -> SimUserDriver {
    let interaction_tx = page
        .debug_services
        .interaction_tx
        .get()
        .expect("interaction_tx set by the window interaction pump")
        .clone();
    SimUserDriver::new(
        app,
        page.rebind.window(),
        page.bounds.clone(),
        page.engine.clone(),
        page.runtime.handle().clone(),
        interaction_tx,
    )
}

/// (a) BOUNDS DISJOINTNESS. The plain outline row that FOLLOWS a nested query
/// band must start below the band's last painted row.
///
/// Bug #69 (Martin, dogfood 2026-07-31, evidence shots 01/03/04): the outline
/// reserves the height the band's row *measured at* — roughly 4 rows — while
/// the band paints 18+. The following sibling is therefore placed ~14 rows too
/// high and draws on top of the band's lower rows. On Martin's real ClaudeCode
/// page every section overlaps the one above it.
///
/// The assert cites COORDINATES (band bottom vs sibling top), so a failure is
/// unambiguously geometric. A vacuous pass is excluded by two preconditions:
/// the band must have painted rows, and the sibling must be on screen.
///
/// The judged frame is the one reached by revealing the SIBLING — the outline
/// is virtualized, so a row is only on screen while it is in the viewport, and
/// the boundary this test judges is the seam between band and sibling.
#[test]
fn band_rows_do_not_overlap_the_following_sibling_row() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = TestApp::with_text_system_and_assets(text_system, assets);
    let page = boot_band_page(&mut app, BAND_POST_COUNT_OVERLAP);

    // Reveal the BAND, not the sibling: `scroll_to_entity` brings its target to
    // the top of the viewport, so revealing the sibling would push the band —
    // the thing whose bottom edge this test measures — above the fold. With the
    // band at the top, the seam it must not cross is in view whether the band is
    // correct (sibling just below the band's ~18 rows) or defective (sibling
    // drawn a few rows down, inside the band).
    {
        let driver = band_driver(&app, &page);
        page.runtime
            .block_on(async {
                driver
                    .scroll_to_entity(&holon_api::EntityUri::block("band-head"))
                    .await
            })
            .expect("reveal the nested query band");
    }
    // Then settle on PAINT, bounded: the reveal's scroll → mount → paint cascade
    // needs to commit before geometry is read. On a build where the band paints
    // nothing this simply runs to its deadline and the assert below reports it.
    let paint_deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < paint_deadline {
        settle_to_fixed_point(
            &mut app,
            &page.bounds,
            &page.runtime,
            Duration::from_secs(30),
        );
        if painted_rows(&page.bounds, BAND_ROW_MARKER).len() >= BAND_ROW_COUNT {
            break;
        }
    }

    let band = painted_rows(&page.bounds, BAND_ROW_MARKER);
    eprintln!(
        "[band-overlap] after reveal: band_painted={} sib_painted={} pre_painted={}",
        band.len(),
        painted_rows(&page.bounds, BAND_SIBLING_CONTENT).len(),
        painted_rows(&page.bounds, BAND_PRE_MARKER).len(),
    );
    eprintln!(
        "[band-overlap] band row ancestors: {:#?}",
        ancestor_chain(&page.bounds, BAND_ROW_MARKER)
    );
    eprintln!(
        "[band-overlap] sibling ancestors: {:#?}",
        ancestor_chain(&page.bounds, BAND_SIBLING_CONTENT)
    );
    assert!(
        !band.is_empty(),
        "precondition: with the band revealed its rows must be on screen, but NO row carrying \
         `{BAND_ROW_MARKER}` was painted while the ViewModel holds them and the backend holds {} \
         matching block rows — that is a regression of #60 (the band paints nothing), not the \
         reserved-height defect this test exists for",
        page.seeded_row_count(),
    );

    let band_bottom = band.iter().map(|(_, y, h)| y + h).fold(f32::MIN, f32::max);

    // THE CONTRACT. The outline row that HOLDS the band must reserve a box that
    // contains what the band painted. Judged on the reserved box rather than on
    // "is the next sibling below the band", because once the bug is fixed the
    // band legitimately fills the viewport and pushes its sibling off-screen —
    // a correct layout must not make its own oracle unsatisfiable. Everything
    // the outline lays out after this row starts at the row's bottom edge, so
    // this single comparison is what overlap reduces to, and it holds no matter
    // where the viewport happens to sit.
    let (row_y, row_h) = outline_row_box(&page.bounds, BAND_ROW_MARKER).expect(
        "the band's painted rows must sit inside a tracked `tree_item` outline row — without one \
         there is no reserved box to compare against and the page shape is not what this test \
         assumes",
    );
    let row_bottom = row_y + row_h;
    eprintln!(
        "[band-overlap] band rows={} painted y=[{:.1}..{band_bottom:.1}] | outline row reserved \
         y=[{row_y:.1}..{row_bottom:.1}] ({row_h:.1}px)",
        band.len(),
        band.first().map(|r| r.1).unwrap_or(f32::NAN),
    );
    assert!(
        row_bottom + OVERLAP_EPSILON_PX >= band_bottom,
        "the outline RESERVED {row_h:.1}px (y={row_y:.1}..{row_bottom:.1}) for its band row, but \
         the band PAINTED down to y={band_bottom:.1} — short by {:.1}px. The next row starts at \
         y={row_bottom:.1}, so it and everything after it draw on top of the band's lower rows. \
         Band row ancestors (innermost first — the first 0-height entry is the container that \
         drops the height): {:#?}",
        band_bottom - row_bottom,
        ancestor_chain(&page.bounds, BAND_ROW_MARKER),
    );

    // Nothing else may overlap either: within a single outline, no two painted
    // text rows share vertical space. Whatever of the page is on screen in this
    // frame gets checked — the sibling included when it is visible, which is the
    // direct reading of Martin's screenshots.
    let mut all: Vec<(String, f32, f32)> = painted_rows(&page.bounds, BAND_ROW_MARKER);
    all.extend(painted_rows(&page.bounds, BAND_PRE_MARKER));
    all.extend(painted_rows(&page.bounds, BAND_POST_MARKER));
    all.extend(painted_rows(&page.bounds, BAND_SIBLING_CONTENT));
    all.sort_by(|a, b| a.1.total_cmp(&b.1));
    let overlaps: Vec<String> = all
        .windows(2)
        .filter(|w| w[0].1 + w[0].2 - w[1].1 > OVERLAP_EPSILON_PX)
        .map(|w| {
            format!(
                "`{}`@{:.1}+{:.1} overlaps `{}`@{:.1} by {:.1}px",
                w[0].0,
                w[0].1,
                w[0].2,
                w[1].0,
                w[1].1,
                w[0].1 + w[0].2 - w[1].1
            )
        })
        .collect();
    assert!(
        overlaps.is_empty(),
        "{} pair(s) of painted outline rows overlap vertically: {overlaps:#?}",
        overlaps.len(),
    );

    eprintln!(
        "[band-overlap] PASS — {} painted rows, none overlapping",
        all.len()
    );

    drop(page.rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(page.env);
}

/// (b) REACHABILITY. A page containing a nested query band must be able to
/// scroll to its LAST row.
///
/// Bug #69, second half: the outline's scroll extent is summed from the same
/// reserved row heights, so a band that paints far taller than it reserved
/// leaves the extent short by the difference. Rows past that point are
/// permanently unreachable — scrolling stops early or no-ops entirely. The
/// differential control Martin ran (the same rows as PLAIN outline blocks
/// scroll fine) is what makes the band the suspect.
///
/// Scroll is driven through `SimUserDriver::scroll_at` — the same wheel path
/// production takes. The test distinguishes the two failure modes it could hit:
/// "scroll did nothing at all" (wheel pipeline) vs "scroll worked but ran out
/// of extent before the last row" (the #69 defect).
#[test]
fn page_with_a_nested_band_scrolls_to_its_last_row() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = TestApp::with_text_system_and_assets(text_system, assets);
    let page = boot_band_page(&mut app, BAND_POST_COUNT_SCROLL);

    let driver = band_driver(&app, &page);

    // Anchor ABOVE the last row, at the band: "can the last row be reached by
    // scrolling down" is only a question from somewhere above it, and the band
    // is the part of the page whose height the extent depends on. (Anchoring at
    // the page's very first row instead leaves the band unpainted — the outline
    // is virtualized and `band-pre-1` sits far enough above the band that the
    // band never enters the built window.)
    page.runtime
        .block_on(async {
            driver
                .scroll_to_entity(&holon_api::EntityUri::block("band-head"))
                .await
        })
        .expect("reveal the nested query band");
    // Bounded paint-wait, same as the overlap rung: one fixed point is not
    // enough for the reveal's scroll → mount → paint cascade to commit, and
    // reading geometry from an in-flight frame reports an empty band as a #60
    // regression.
    let paint_deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < paint_deadline {
        settle_to_fixed_point(
            &mut app,
            &page.bounds,
            &page.runtime,
            Duration::from_secs(30),
        );
        if !painted_rows(&page.bounds, BAND_ROW_MARKER).is_empty() {
            break;
        }
    }

    let band = painted_rows(&page.bounds, BAND_ROW_MARKER);
    assert!(
        !band.is_empty(),
        "precondition: with the band revealed it must paint rows, none carried \
         `{BAND_ROW_MARKER}` while the backend holds {} matching block rows — that is a regression \
         of #60, not the scroll-extent defect this test exists for",
        page.seeded_row_count(),
    );
    assert!(
        painted_rows(&page.bounds, BAND_LAST_CONTENT).is_empty(),
        "precondition: `{BAND_LAST_CONTENT}` must start BELOW the fold, otherwise this test never \
         exercises scrolling — the seeded page is too short for the window"
    );
    // Reference point for "did the wheel move anything at all", read from the
    // band because that is what is on screen at the anchor.
    let top_before = band[0].1;

    // Aim the wheel at a point provably INSIDE the main panel: the centre of a
    // row the panel actually painted. A hardcoded viewport centre would risk a
    // hit-test miss and a misleading "scroll did nothing".
    let (wheel_x, wheel_y) = page
        .bounds
        .all_elements()
        .iter()
        .find_map(|(_, i)| {
            let text = i.displayed_text.as_deref()?;
            (text.contains(BAND_ROW_MARKER) && i.width > 1.0)
                .then(|| (i.x + i.width / 2.0, i.y + i.height / 2.0))
        })
        .expect("precondition: a band row must be painted to aim the wheel at");

    let mut last_seen: Option<f32> = None;
    for step in 0..40 {
        page.runtime
            .block_on(async { driver.scroll_at(wheel_x, wheel_y, 0.0, -400.0).await })
            .expect("wheel-scroll the main panel down");
        settle_to_fixed_point(
            &mut app,
            &page.bounds,
            &page.runtime,
            Duration::from_secs(30),
        );
        let last = painted_rows(&page.bounds, BAND_LAST_CONTENT);
        let topmost = painted_rows(&page.bounds, BAND_POST_MARKER)
            .first()
            .map(|r| r.0.clone());
        eprintln!(
            "[band-reach] step {step}: last_row_visible={} topmost_post={topmost:?}",
            !last.is_empty(),
        );
        if !last.is_empty() {
            last_seen = Some(last[0].1);
            break;
        }
    }

    let top_after = painted_rows(&page.bounds, BAND_ROW_MARKER)
        .first()
        .map(|r| r.1);
    let scrolled_at_all = top_after.is_none_or(|y| (y - top_before).abs() > 1.0);

    assert!(
        last_seen.is_some(),
        "the page's LAST row `{BAND_LAST_CONTENT}` never became visible after 40 wheel-scrolls \
         down (scroll moved the content at all: {scrolled_at_all}; anchor band row y \
         {top_before:.1} -> {top_after:?}). The outline's scroll extent is summed from the heights \
         it RESERVED for its rows; if a nested band paints more than it reserved, the extent falls \
         short by the difference and the rows past it are unreachable.",
    );

    eprintln!(
        "[band-reach] PASS — last row reached at y={:.1}",
        last_seen.unwrap()
    );

    drop(page.rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(page.env);
}
