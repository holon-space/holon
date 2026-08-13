//! A-lane increment A1: the windowed rung that lets
//! `inv-journal-feed-viewport-lazy` reach a verdict.
//!
//! The invariant is windowed-only (it reads painted geometry to learn what is
//! on screen) and it only applies while `block:journals` IS the rendered Main
//! focus root. No other windowed rung ever puts it there — the windowed seed
//! has no journals topology at all — so without this file the invariant skips
//! everywhere. See `~/.claude/plans/holon-viewport-expansion-plan.md` §3 A1.
//!
//! Lives in its own binary rather than beside the other windowed slices in
//! `gpui_window_slice.rs`: that file does not compile against the current gpui
//! pin (it drives `SimUserDriver::new` with a `TestApp` where the harness now
//! takes a `HeadlessAppContext`), and repairing three unrelated tests is not
//! this increment's change.
//!
//! ⚠ Run with `--test-threads=1`: the gpui test platform holds thread-local
//! state and two windowed tests in one process abort.

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use gpui::PlatformTextSystem;
use holon_api::EntityUri;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::user_driver::UserDriver;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::composed::composed_invariant_catalog;
use holon_integration_tests::pbt::window_slice::builders::window_ref_caps_journal_feed;
use holon_integration_tests::pbt::window_slice::builders::window_wide;
use holon_integration_tests::pbt::window_slice::seed::JOURNAL_DAY_COUNT;
use holon_integration_tests::pbt::window_slice::seed::JOURNALS_ID;
use holon_integration_tests::pbt::window_slice::seed::graft_journal_days;
use holon_integration_tests::test_environment::TestEnvironment;
use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::composition::run_selected;

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;
use pbt_harness::sim_windowed_replay::SimUserDriver;

fn real_text_system() -> Arc<dyn PlatformTextSystem> {
    gpui_platform::current_platform(true).text_system()
}

/// Cross-runtime fixed-point settle (the shared windowed pattern): pump until
/// the element count is stable and no `"loading"` placeholders remain.
fn settle_to_fixed_point(
    app: &mut HeadlessAppContext,
    bounds: &BoundsRegistry,
    runtime: &tokio::runtime::Runtime,
    timeout: Duration,
) {
    let start = Instant::now();
    let mut last_count = 0usize;
    let mut stable_iters = 0u32;
    let mut trajectory: Vec<(usize, bool)> = Vec::new();
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
        trajectory.push((count, still_loading));
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
    // The trajectory separates the two ways this times out: a count that keeps
    // moving (the frame is still changing) from one that is stable but never
    // sheds its `loading` placeholders.
    let tail: Vec<String> = trajectory
        .iter()
        .rev()
        .take(30)
        .rev()
        .map(|(c, l)| if *l { format!("{c}L") } else { c.to_string() })
        .collect();
    panic!(
        "window never reached a fixed point within {timeout:?}: {} elements; last counts \
         (L = loading present): {}",
        bounds.all_elements().len(),
        tail.join(" "),
    );
}

/// A journals feed must expand exactly the day pages inside the rendered
/// window.
///
/// RED until A2 lands the viewport gate: the feed default-expands every day it
/// draws, so days far below the fold are expanded — each one materialising a
/// `live_query` shell, a watched matview and a CDC subscription for content
/// nobody can see, which is what makes feed cost scale with history.
///
/// The on-screen assertion is the attribution control: it must PASS in the same
/// run, so the failure belongs to the off-screen arm alone.
#[test]
#[ignore = "red-first evidence for increment A2 (viewport-driven expansion): fails on the current tree BY DESIGN — 40 of 70 feed day pages expand off-screen. Red log: lane-logs/a1-RED-inv-journal-feed-viewport-lazy.log. A2's first step is removing this ignore."]
fn journals_feed_expands_only_the_day_pages_on_screen() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

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
    let rebind = app
        .update(|cx| {
            launch_holon_window_rebindable(
                session.clone(),
                engine.clone(),
                runtime.handle().clone(),
                nav,
                bounds.clone(),
                Some(debug_services.clone()),
                None,
                "Holon-Journals-Viewport",
                cx,
            )
        })
        .expect("window opened");
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let days = runtime
        .block_on(graft_journal_days(&env, JOURNAL_DAY_COUNT))
        .expect("graft the journals day pages");
    // A hundred new pages are a hundred new documents: let the CDC/org-writeback
    // wave finish before asking the window for a stable frame, or the settle
    // chases a backend that is still moving.
    runtime
        .block_on(env.wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(120)));
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(180));

    // Navigate through the sidebar, not a synthesized dispatch: the click's
    // bound `navigation.focus` is what writes `focus_roots`, and the panel
    // renders that table's row.
    let journals = EntityUri::block(JOURNALS_ID);
    let interaction_tx = debug_services
        .interaction_tx
        .get()
        .expect("interaction_tx set by the window interaction pump")
        .clone();
    let app_ptr: *const HeadlessAppContext = &app;
    let driver = SimUserDriver::new(
        app_ptr,
        rebind.window(),
        bounds.clone(),
        engine.clone(),
        runtime.handle().clone(),
        interaction_tx,
    );
    runtime
        .block_on(async { driver.click_entity(&journals, "left_sidebar").await })
        .expect("click the journals page in the sidebar to focus the Main panel on it");
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(120));

    let focus_rows = runtime
        .block_on(env.query_sql("SELECT root_id FROM focus_roots WHERE region = 'main'"))
        .expect("read the Main focus root");
    let focused_root = focus_rows
        .first()
        .and_then(|r| r.get("root_id"))
        .and_then(|v| v.as_string().map(str::to_string));
    assert_eq!(
        focused_root.as_deref(),
        Some(journals.as_str()),
        "the sidebar click must land the Main focus root on {} — every assertion below reads the \
         feed the panel draws for that root",
        journals.as_str(),
    );

    let sut = window_wide(Box::new(bounds.clone()), engine.clone());
    let snapshot = runtime.block_on(async { sut.widget_tree_snapshot().await });
    let elements = runtime.block_on(async { sut.rendered_elements().await });

    // The main panel's box is the viewport. Classification mirrors the oracle's
    // two-signal rule (POSITION and CLIP, see the invariant's module docs) so
    // this independent read can corroborate its verdict rather than measure a
    // different thing and agree by luck.
    let panel = elements
        .iter()
        .find(|e| {
            e.entity_id.as_ref().map(EntityUri::as_str) == Some("block:default-main-panel")
                && e.height > 1.0
        })
        .expect("the main panel must register a box — it is the feed's viewport");
    let panel_top = panel.y;
    let panel_bottom = panel.y + panel.height;
    let ids_of = |pred: &dyn Fn(&holon_pbt_core::capabilities::RenderedElement) -> bool| {
        elements
            .iter()
            .filter(|e| pred(e))
            .filter_map(|e| e.entity_id.as_ref())
            .map(EntityUri::as_str)
            .collect::<std::collections::BTreeSet<&str>>()
    };
    let by_position = ids_of(&|e| e.y >= panel_top && e.y < panel_bottom);
    let by_clip = ids_of(&|e| e.height > 0.0);
    let toggles = snapshot.collect_by_kind("expand_toggle");
    let expanded = |day: &str| -> bool {
        toggles
            .iter()
            .find(|t| t.props.get("target_id").map(String::as_str) == Some(day))
            .and_then(|t| t.props.get("expanded"))
            .map(String::as_str)
            == Some("true")
    };

    // Feed order is newest-first (`ORDER BY content DESC`), the reverse of the
    // fixture's creation order.
    let feed: Vec<String> = days
        .iter()
        .rev()
        .map(|(id, _)| EntityUri::block(id).as_str().to_string())
        .collect();
    let onscreen: Vec<&String> = feed
        .iter()
        .filter(|d| by_position.contains(d.as_str()) && by_clip.contains(d.as_str()))
        .collect();
    let offscreen: Vec<&String> = feed
        .iter()
        .filter(|d| !by_position.contains(d.as_str()) && !by_clip.contains(d.as_str()))
        .collect();
    let undecided: Vec<&String> = feed
        .iter()
        .filter(|d| by_position.contains(d.as_str()) != by_clip.contains(d.as_str()))
        .collect();
    let offscreen_expanded: Vec<&String> =
        offscreen.iter().filter(|d| expanded(d)).copied().collect();
    let onscreen_collapsed: Vec<&String> =
        onscreen.iter().filter(|d| !expanded(d)).copied().collect();

    eprintln!(
        "[journals-viewport] days={} onscreen={} offscreen={} undecided={undecided:?} toggles={} \
         expanded_offscreen={} collapsed_onscreen={}",
        feed.len(),
        onscreen.len(),
        offscreen.len(),
        toggles.len(),
        offscreen_expanded.len(),
        onscreen_collapsed.len(),
    );

    // Registration is not visibility: the feed registers bounds for every day
    // it lays out, so the extent below runs well past the panel's bottom edge.
    // This line is what tells a reader the fixture really overflowed the
    // viewport rather than the classification being an artefact.
    let laid_out_bottom = elements
        .iter()
        .filter(|e| {
            feed.iter()
                .any(|d| Some(d.as_str()) == e.entity_id.as_ref().map(EntityUri::as_str))
        })
        .map(|e| e.y + e.height)
        .fold(0.0f32, f32::max);
    eprintln!(
        "[journals-viewport] panel box y {panel_top:.0}..{panel_bottom:.0}; day rows laid out \
         down to y {laid_out_bottom:.0}",
    );

    // VACUITY GUARDS. The oracle below can reach `Ok` without judging anything —
    // an empty feed, an unreadable expansion state, or a frame with no day left
    // off screen all produce a green that means nothing. These pin the evidence
    // the verdict has to have been drawn from, so a green verdict after A2
    // cannot be confused with an unexercised one.
    assert!(
        !onscreen.is_empty(),
        "the panel must paint at least one of the {} grafted day pages, else the claim below is \
         vacuous",
        feed.len(),
    );
    assert!(
        !toggles.is_empty(),
        "the panel must draw expand toggles for the day pages, else expansion is unreadable from \
         this frame",
    );
    // Mirrors the oracle's refusal. Without it this read would go quiet on
    // exactly the frames the oracle refuses — a frontend that stops clipping
    // off-screen rows makes every leaked day undecided here too, and the rung
    // would corroborate a green it never checked.
    assert!(
        undecided.is_empty(),
        "POSITION and CLIP disagree about {} day page(s): {undecided:?}. The frame is undecidable, \
         so neither this read nor the oracle may judge it",
        undecided.len(),
    );
    assert!(
        onscreen.len() < feed.len(),
        "the fixture must leave day pages OFF screen — all {} are on screen, so the off-screen arm \
         has nothing to judge and this rung cannot exercise its claim",
        feed.len(),
    );
    // The oracle refuses to judge a frame whose whole feed fits the panel, so
    // the fixture must out-size the panel's capacity, not merely its visible
    // count. Mirrors `onscreen_capacity` at its 16px row floor.
    let capacity = ((panel_bottom - panel_top) / 16.0).ceil() as usize;
    assert!(
        feed.len() > capacity,
        "the fixture's {} day(s) must exceed the panel's {capacity}-row capacity, else a SUT \
         claiming the whole feed is on screen stays under the bounded law and blinds the oracle",
        feed.len(),
    );

    // ATTRIBUTION CONTROL: the on-screen arm passes today, so the failure below
    // is the off-screen arm's alone.
    assert!(
        onscreen_collapsed.is_empty(),
        "on-screen day pages must be EXPANDED; these are collapsed: {onscreen_collapsed:?}",
    );

    // THE CLAIM, as the oracle states it (RED until A2's viewport gate).
    let ref_caps = window_ref_caps_journal_feed(&days);
    let report = runtime.block_on(run_selected(&composed_invariant_catalog(), &sut, &ref_caps));
    let inv_id = "inv-journal-feed-viewport-lazy";
    eprintln!(
        "[journals-viewport] {inv_id} => {:?}",
        report
            .ran
            .iter()
            .find(|(id, _)| id.0 == inv_id)
            .map(|(_, r)| r),
    );
    assert!(
        report.ran_ids().contains(&inv_id),
        "the journals rung must SELECT {inv_id}; ran={:?}",
        report.ran_ids(),
    );
    // Only the invariant under test is asserted. The rest of the catalog also
    // selects here, but this rung's ref is hand-seeded with the journals
    // topology alone, so their failures report ref fidelity, not the claim.
    eprintln!(
        "[journals-viewport] other failures (ref-fidelity, not this claim): {:?}",
        report
            .failures()
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| *id != inv_id)
            .collect::<Vec<_>>(),
    );
    let verdict = report
        .ran
        .iter()
        .find(|(id, _)| id.0 == inv_id)
        .map(|(_, r)| r)
        .expect("the invariant under test must be in the ran set");
    assert!(
        matches!(verdict, holon_pbt_core::invariant::InvariantResult::Ok),
        "{inv_id} must reach Ok over the journals window. A Skip is NOT a pass: it means the \
         oracle refused to judge — the fixture never put the feed in front of it, or the frame \
         was undecidable. Got: {verdict:?}",
    );

    // THE CLAIM, read straight off the frame. Corroborates the oracle's
    // verdict above from an independent computation of the same fact.
    assert!(
        offscreen_expanded.is_empty(),
        "{} of {} feed day page(s) are EXPANDED while OFF SCREEN, e.g. {:?}. Each materialises a \
         live_query shell, a watched matview and a CDC subscription for content nobody can see, so \
         feed cost scales with history instead of with the window. On screen: {} day(s).",
        offscreen_expanded.len(),
        feed.len(),
        offscreen_expanded.iter().take(5).collect::<Vec<_>>(),
        onscreen.len(),
    );

    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(env);
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
