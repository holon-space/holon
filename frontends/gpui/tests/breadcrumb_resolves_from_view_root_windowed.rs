//! The breadcrumb bar shows the path of the page the user is LOOKING AT, not
//! only of a block they have put the caret in.
//!
//! Both chrome bars sit ABOVE the main panel, so a bar that resolves late
//! inserts ~31px and reflows the panel mid-gesture — on the frame the soft
//! keyboard is also shrinking it (the sibling escape
//! `docs/Testing/bugfunnel/entries/2026-08-28-tab-strip-never-resolves-at-boot.
//! md`). The cold-boot leg is therefore the load-bearing one.
//!
//! Run: `cargo nextest run -p holon-gpui --test
//! breadcrumb_resolves_from_view_root_windowed --features holon-gpui/pbt`

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use gpui::PlatformTextSystem;
use holon_api::EntityUri;
use holon_api::Value;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::user_driver::UserDriver;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::window_slice::seed::graft_displayed_text_tree;
use holon_integration_tests::test_environment::TestEnvironment;

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;
use pbt_harness::sim_windowed_replay::SimUserDriver;

/// The row `graft_displayed_text_tree` hangs `c1`/`c2` under. Navigating Main
/// onto it is a zoom-in: a view root that is NOT itself a page, so the trail
/// still has to come from its page ancestor.
const ZOOM_ROW: &str = "parent";
/// The row the test taps, so the focused-block leg is a plain caret gesture.
const TAPPED_ROW: &str = "c1";

/// An OPEN main row the cursor is NOT sitting on: closing it moves no view.
const BACKGROUND_TAB_SQL: &str = "SELECT nh.id AS id FROM navigation_history nh WHERE nh.region = \
                                  'main' AND nh.closed_at IS NULL AND nh.id <> (SELECT \
                                  nc.history_id FROM navigation_cursor nc WHERE nc.region = \
                                  'main') LIMIT 1";

const ACTIVE_MAIN_ROOT_SQL: &str = "SELECT fr.root_id FROM focus_roots fr JOIN navigation_cursor \
                                    nc ON nc.history_id = fr.history_id WHERE fr.region = 'main' \
                                    AND nc.region = 'main'";

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
        bounds.all_elements().len(),
    );
}

/// The Main region's view root, read from the tables the panel itself is a
/// view of — the oracle the bar is judged against.
fn main_root_rows(
    env: &TestEnvironment,
    runtime: &tokio::runtime::Runtime,
) -> Vec<holon_api::widget_spec::DataRow> {
    runtime
        .block_on(env.query_sql(ACTIVE_MAIN_ROOT_SQL))
        .expect("read the active Main root")
}

fn active_main_root(env: &TestEnvironment, runtime: &tokio::runtime::Runtime) -> EntityUri {
    let rows = main_root_rows(env, runtime);
    let raw = rows
        .first()
        .and_then(|r| r.get("root_id"))
        .and_then(|v| v.as_string())
        .expect(
            "the Main region must have an active root, else every claim below about the bar \
             following it is vacuous",
        )
        .to_string();
    EntityUri::parse(&raw).expect("focus_roots.root_id is a block URI")
}

/// Navigate through the production chokepoint the sidebar, quick-open and
/// wiki-links all dispatch through (`search_ui::navigate_to`'s intent).
fn navigate_main_to(services: &Arc<dyn BuilderServices>, target: &EntityUri) {
    services.dispatch_intent(OperationIntent::new(
        "navigation".into(),
        "focus".into(),
        [
            ("region".to_string(), Value::String("main".to_string())),
            ("block_id".to_string(), Value::String(target.to_string())),
        ]
        .into_iter()
        .collect(),
    ));
}

fn dispatch(services: &Arc<dyn BuilderServices>, op: &str, params: Vec<(&str, Value)>) {
    services.dispatch_intent(OperationIntent::new(
        "navigation".into(),
        op.to_string(),
        params
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    ));
}

/// Open MAIN tabs as `(history_id, root_id)`, insertion-ordered — the rows the
/// strip and `navigation.activate` address.
fn open_main_tabs(
    env: &TestEnvironment,
    runtime: &tokio::runtime::Runtime,
) -> Vec<(i64, EntityUri)> {
    runtime
        .block_on(env.query_sql(
            "SELECT history_id, root_id FROM focus_roots WHERE region = 'main' ORDER BY \
             history_id",
        ))
        .expect("read the open Main tabs")
        .iter()
        .map(|row| {
            let id = row
                .get("history_id")
                .and_then(|v| v.as_i64())
                .expect("focus_roots.history_id is an integer");
            let raw = row
                .get("root_id")
                .and_then(|v| v.as_string())
                .expect("focus_roots.root_id is a string");
            (id, EntityUri::parse(raw).expect("root_id is a block URI"))
        })
        .collect()
}

fn background_tab_id(env: &TestEnvironment, runtime: &tokio::runtime::Runtime) -> i64 {
    runtime
        .block_on(env.query_sql(BACKGROUND_TAB_SQL))
        .expect("read a background Main tab")
        .first()
        .and_then(|row| row.get("id"))
        .and_then(|v| v.as_i64())
        .expect("a second open Main tab to close in the background")
}

#[test]
fn the_breadcrumb_follows_the_view_root_and_then_the_focus() {
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
                "Holon-Breadcrumb-View-Root",
                cx,
            )
        })
        .expect("window opened");
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    // THE COLD-BOOT CLAIM. Nothing has been navigated, tapped or typed yet:
    // this is the frame the user is handed when the app opens.
    assert_eq!(
        engine.ui_state().focused_block(),
        None,
        "a cold boot must leave nothing focused, else this is the focused-block case and proves \
         nothing about the view root",
    );

    let root = active_main_root(&env, &runtime);
    let (block, segments) = app.update(|cx| {
        (
            rebind.breadcrumb_block(cx),
            rebind.drawn_breadcrumb_segments(cx),
        )
    });
    eprintln!("[breadcrumb-view-root/boot] root={root} bar_block={block:?} segments={segments:?}");

    assert_eq!(
        block.as_ref(),
        Some(&root),
        "with nothing focused the breadcrumb bar must show the path of the page Main is open on \
         ({root}); it shows {block:?}. Until the user's first tap the bar draws nothing and then \
         appears all at once, reflowing the panel under their finger.",
    );
    assert!(
        segments.is_some_and(|n| n > 0),
        "the bar resolved {root} but drew {segments:?} segments — a bar with no segments renders \
         nothing, which is the pop-in this test exists to prevent",
    );

    // Give the outline something to zoom into and tap.
    runtime
        .block_on(graft_displayed_text_tree(&env))
        .expect("graft a page and navigate Main onto it");
    runtime
        .block_on(env.wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)));
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(120));

    // Navigating Main moves the bar with the view.
    let services: Arc<dyn BuilderServices> = engine.clone();
    let zoom = EntityUri::block(ZOOM_ROW);
    navigate_main_to(&services, &zoom);
    runtime
        .block_on(env.wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)));
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(120));

    let zoomed_root = active_main_root(&env, &runtime);
    assert_eq!(
        zoomed_root, zoom,
        "the zoom navigation must land Main on {zoom}",
    );
    let (block, segments) = app.update(|cx| {
        (
            rebind.breadcrumb_block(cx),
            rebind.drawn_breadcrumb_segments(cx),
        )
    });
    eprintln!(
        "[breadcrumb-view-root/zoomed] root={zoomed_root} bar_block={block:?} \
         segments={segments:?}"
    );
    assert_eq!(
        block.as_ref(),
        Some(&zoom),
        "navigating Main to {zoom} must move the breadcrumb with it; the bar still shows \
         {block:?}",
    );
    assert!(
        segments.is_some_and(|n| n > 0),
        "the zoomed-in row is not itself a page, so its trail is its page ancestor — drew \
         {segments:?} segments",
    );

    // Focusing a block hands the bar back to that block: the behaviour the bar
    // already had, unchanged.
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
    let tapped = EntityUri::block(TAPPED_ROW);
    runtime
        .block_on(async { driver.click_entity(&tapped, "main").await })
        .expect("tap the outline row to put the caret in it");
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(120));

    assert_eq!(
        engine.ui_state().focused_block().as_ref(),
        Some(&tapped),
        "the tap must actually move the focus, else the claim below is vacuous",
    );
    let block = app.update(|cx| rebind.breadcrumb_block(cx));
    eprintln!("[breadcrumb-view-root/focused] tapped={tapped} bar_block={block:?}");
    assert_eq!(
        block.as_ref(),
        Some(&tapped),
        "a focused block owns the bar: it must show {tapped}'s trail, not {block:?}",
    );

    // BACK. It moves the region cursor without naming a target, so it neither
    // sets the focus nor counts as a page change — the caret stays on the row
    // the user left behind, on a page that is no longer displayed. The bar
    // belongs to the view, so it must follow Back rather than keep drawing the
    // departed page's trail.
    dispatch(
        &services,
        "go_back",
        vec![("region", Value::String("main".into()))],
    );
    runtime
        .block_on(env.wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)));
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(120));

    let back_root = active_main_root(&env, &runtime);
    assert_ne!(
        back_root, zoom,
        "Back must actually move the Main view off {zoom}, else this leg is vacuous",
    );
    let (block, segments) = app.update(|cx| {
        (
            rebind.breadcrumb_block(cx),
            rebind.drawn_breadcrumb_segments(cx),
        )
    });
    eprintln!(
        "[breadcrumb-view-root/back] root={back_root} bar_block={block:?} segments={segments:?} \
         focus={:?}",
        engine.ui_state().focused_block(),
    );
    assert_eq!(
        block.as_ref(),
        Some(&back_root),
        "after Back the Main view shows {back_root}; the bar shows {block:?}. A bar left on the \
         departed page is worse than an empty one — it names a page the user is no longer on.",
    );
    assert!(
        segments.is_some_and(|n| n > 0),
        "the returned-to view drew {segments:?} segments",
    );

    // TAB SWITCH. `activate` moves the cursor between already-open tabs, again
    // without touching the focus.
    dispatch(
        &services,
        "open_tab",
        vec![
            ("region", Value::String("main".into())),
            ("block_id", Value::String(zoom.to_string())),
        ],
    );
    runtime
        .block_on(env.wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)));
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(120));

    let tabs = open_main_tabs(&env, &runtime);
    let current = active_main_root(&env, &runtime);
    let (target_id, target_root) = tabs
        .iter()
        .find(|(_, root)| root != &current)
        .cloned()
        .unwrap_or_else(|| {
            panic!("need a second open tab to switch to; open tabs are {tabs:?}, current {current}")
        });

    dispatch(
        &services,
        "activate",
        vec![
            ("region", Value::String("main".into())),
            ("history_id", Value::Integer(target_id)),
        ],
    );
    runtime
        .block_on(env.wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)));
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(120));

    let switched_root = active_main_root(&env, &runtime);
    assert_eq!(
        switched_root, target_root,
        "the tab switch must land Main on {target_root}, else this leg is vacuous",
    );
    let block = app.update(|cx| rebind.breadcrumb_block(cx));
    eprintln!(
        "[breadcrumb-view-root/activate] root={switched_root} bar_block={block:?} focus={:?}",
        engine.ui_state().focused_block(),
    );
    assert_eq!(
        block.as_ref(),
        Some(&switched_root),
        "switching tabs must move the bar to the switched-to tab ({switched_root}); it shows \
         {block:?}",
    );

    // A BACKGROUND tab closing moves no view and no caret, so the bar must not
    // move either. `navigation.close` carries no region, so the mirror bumps the
    // view generation for it regardless — the bar must survive that.
    let recaret = EntityUri::block(TAPPED_ROW);
    runtime
        .block_on(async { driver.click_entity(&recaret, "main").await })
        .expect("put the caret back in an outline row");
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(120));
    let bar_before_close = app.update(|cx| rebind.breadcrumb_block(cx));
    assert_eq!(
        bar_before_close.as_ref(),
        Some(&recaret),
        "the caret must own the bar before the close, else this leg is vacuous",
    );

    let root_before_close = active_main_root(&env, &runtime);
    let background = background_tab_id(&env, &runtime);
    let gen_before = engine.ui_state().main_view_generation();
    dispatch(
        &services,
        "close",
        vec![("history_id", Value::Integer(background))],
    );
    runtime
        .block_on(env.wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)));
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(120));

    let root_after_close = active_main_root(&env, &runtime);
    let block = app.update(|cx| rebind.breadcrumb_block(cx));
    eprintln!(
        "[breadcrumb-view-root/bg-close] root={root_before_close}->{root_after_close} \
         bar_block={block:?} view_gen={gen_before}->{} focus={:?}",
        engine.ui_state().main_view_generation(),
        engine.ui_state().focused_block(),
    );
    assert_eq!(
        root_after_close, root_before_close,
        "closing a BACKGROUND tab must not move the Main view root, else this leg is vacuous",
    );
    assert_eq!(
        engine.ui_state().focused_block().as_ref(),
        Some(&recaret),
        "closing a background tab must not move the caret, else this leg is vacuous",
    );
    assert_eq!(
        block.as_ref(),
        Some(&recaret),
        "closing a background tab moved neither the view nor the caret, so the bar must stay on \
         {recaret}; it shows {block:?}. A view-generation bump alone is not a reason to steal the \
         bar from a live caret.",
    );

    // Going home clears both the focus and the Main view root, so there is no
    // path left to show. The bar must let go of the block the user left rather
    // than keep drawing its trail over the root view.
    services.dispatch_intent(OperationIntent::new(
        "navigation".into(),
        "go_home".into(),
        [("region".to_string(), Value::String("main".to_string()))]
            .into_iter()
            .collect(),
    ));
    runtime
        .block_on(env.wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)));
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(120));

    assert_eq!(
        engine.ui_state().focused_block(),
        None,
        "go_home must clear the focus, else this leg never reaches the view-root fallback",
    );
    assert!(
        main_root_rows(&env, &runtime).is_empty(),
        "go_home must leave Main with no view root, else the claim below is about the wrong \
         empty state",
    );
    let (block, segments) = app.update(|cx| {
        (
            rebind.breadcrumb_block(cx),
            rebind.drawn_breadcrumb_segments(cx),
        )
    });
    eprintln!("[breadcrumb-view-root/home] bar_block={block:?} segments={segments:?}");
    assert_eq!(
        block, None,
        "with no focus and no view root the bar has no path to show; it still shows {block:?}",
    );
    assert_eq!(
        segments,
        Some(0),
        "a bar with nothing to resolve must draw no segments",
    );

    drop(driver);
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(env);
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
#[path = "test_init/mod.rs"]
mod test_init;
