//! On a phone the left sidebar floats OVER the page. It has to get out of the
//! way by itself: once the user has tapped a page in it, or tapped the page
//! behind it, leaving it up hides the content they just asked for behind an
//! opaque panel with no visible way back.
//!
//! A sidebar that is BESIDE the page instead of over it must survive both
//! gestures — the desktop rung — and so must a panel that merely sits next to
//! an overlay drawer rather than under it: at 600..1000px the bundled layout
//! renders the left sidebar in flow and only the right one floating, and a
//! click in that left sidebar belongs to the sidebar.
//!
//! Run: `cargo nextest run -p holon-gpui --test
//! overlay_sidebar_dismisses_windowed --features holon-gpui/pbt`

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use gpui::PlatformTextSystem;
use holon_api::EntityUri;
use holon_api::Value;
use holon_frontend::geometry::ElementInfo;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::geometry::drawer_toggle_id_for;
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::user_driver::UserDriver;
use holon_frontend::view_model::DrawerMode;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::window_slice::seed::graft_displayed_text_tree;
use holon_integration_tests::test_environment::TestEnvironment;

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;
use pbt_harness::sim_windowed_replay::SimUserDriver;

const LEFT_SIDEBAR: &str = "block:default-left-sidebar";
const RIGHT_SIDEBAR: &str = "block:default-right-sidebar";

/// Under 600px both sidebars resolve to `Overlay` (`Perspective::layout_dsl`).
/// A DN2103 is 412 logical px wide.
const PHONE_WINDOW: &str = "412x915";
/// 600..1000px: the left sidebar is in flow (`Shrink`), the right one floats
/// (`Overlay`) — the band where a scrim inset only by overlay drawers would be
/// painted across the left sidebar.
const MID_WINDOW: &str = "900x900";
/// Above 1000px both sidebars are in flow.
const DESKTOP_WINDOW: &str = "1440x900";

/// The page `graft_displayed_text_tree` hangs rows under — the navigation
/// target standing in for a page tapped in the sidebar.
const NAV_TARGET: &str = "parent";
/// A row the tap-beside-the-sidebar leg aims at, so the dismissing tap has a
/// focus-taking target under it rather than empty panel.
const ROW_UNDER_SCRIM: &str = "block:c2";
/// The row that leg focuses FIRST, to prove the tap point is live at all.
const ROW_CONTROL: &str = "block:c1";

/// The block the `main` region is open on — the oracle the auto-close is
/// judged against, read from the tables the panel is a view of.
const ACTIVE_MAIN_ROOT_SQL: &str = "SELECT fr.root_id FROM focus_roots fr JOIN navigation_cursor \
                                    nc ON nc.history_id = fr.history_id WHERE fr.region = 'main' \
                                    AND nc.region = 'main'";
/// An OPEN main row the cursor is NOT sitting on: closing it moves no view.
const BACKGROUND_TAB_SQL: &str = "SELECT nh.id AS id FROM navigation_history nh WHERE nh.region = \
                                  'main' AND nh.closed_at IS NULL AND nh.id <> (SELECT \
                                  nc.history_id FROM navigation_cursor nc WHERE nc.region = \
                                  'main') LIMIT 1";

/// The row constants carry their `block:` scheme because that is how the bounds
/// registry records `entity_id`.
fn row_uri(row: &str) -> EntityUri {
    EntityUri::parse(row).expect("row constants are block URIs")
}

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

/// Navigate through the production chokepoint the sidebar's page rows,
/// quick-open and wiki-links all dispatch through (`search_ui::navigate_to`).
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

/// `HeadlessAppContext` panics on drop if any entity handle outlives it, and
/// fields drop in declaration order — so `app` comes last, after `rebind`.
struct Harness {
    env: TestEnvironment,
    runtime: Arc<tokio::runtime::Runtime>,
    bounds: BoundsRegistry,
    rebind: holon_gpui::RebindHandle,
    engine: Arc<holon_frontend::reactive::ReactiveEngine>,
    session: Arc<holon_frontend::FrontendSession>,
    interaction_tx: futures::channel::mpsc::Sender<holon_mcp::server::InteractionCommand>,
    app: HeadlessAppContext,
}

/// Open a real window of `window_size`. The breakpoint has to come from the
/// window itself: `observe_window_bounds` is what drives `ViewportInfo`, and
/// only a real size also gives the drawers and the scrim their real geometry.
fn boot(title: &'static str, window_size: &str) -> Harness {
    // Read by `launch_holon_window_impl`; must be set before the window opens.
    // SAFETY: single-threaded test setup, before any window or runtime thread
    // reads the environment.
    unsafe { std::env::set_var("HOLON_INITIAL_WINDOW_SIZE", window_size) };

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
                title,
                cx,
            )
        })
        .expect("window opened");
    // 60s, not the 30s the neighbouring windowed tests use: a pump-free build
    // of this binary was measured at 31.4s to first fixed point on a loaded
    // machine, so 30 is inside the noise.
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(60));

    // Give the main region a page with rows to navigate onto and tap.
    runtime
        .block_on(graft_displayed_text_tree(&env))
        .expect("graft a page and navigate Main onto it");
    runtime
        .block_on(env.wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)));
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(120));

    let interaction_tx = debug_services
        .interaction_tx
        .get()
        .expect("interaction_tx set by the window interaction pump")
        .clone();

    Harness {
        app,
        env,
        runtime,
        bounds,
        rebind,
        engine,
        session,
        interaction_tx,
    }
}

impl Harness {
    /// Shut the window down the way every windowed test does: detached pumps
    /// keep entity handles alive past the last assertion, and
    /// `HeadlessAppContext`'s drop treats those as leaks.
    fn teardown(mut self, driver: SimUserDriver) {
        drop(driver);
        self.app.update(|cx| cx.shutdown());
        self.app.run_until_parked();
        std::mem::forget(self);
    }

    /// The driver holds a raw pointer to `self.app`, so it may only be built
    /// once the harness sits at the address it will keep.
    fn driver(&self) -> SimUserDriver {
        SimUserDriver::new(
            &self.app as *const HeadlessAppContext,
            self.rebind.window(),
            self.bounds.clone(),
            self.engine.clone(),
            self.runtime.handle().clone(),
            self.interaction_tx.clone(),
        )
    }

    fn settle(&mut self) {
        settle_to_fixed_point(
            &mut self.app,
            &self.bounds,
            &self.runtime,
            Duration::from_secs(120),
        );
    }

    fn navigate_to(&mut self, target: &EntityUri) {
        let services: Arc<dyn BuilderServices> = self.engine.clone();
        navigate_main_to(&services, target);
        self.runtime.block_on(
            self.env
                .wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)),
        );
        self.settle();
    }

    /// The mode a sidebar's drawer resolved to in the drawn tree.
    fn mode(&mut self, sidebar: &str) -> DrawerMode {
        let Harness { app, rebind, .. } = self;
        let drawers = app.update(|cx| rebind.drawers(cx));
        drawers
            .iter()
            .find(|(id, _)| id == sidebar)
            .map(|(_, mode)| *mode)
            .unwrap_or_else(|| {
                panic!("the root view must draw a {sidebar} drawer; drew {drawers:?}")
            })
    }

    fn is_open(&self, sidebar: &str, mode: DrawerMode) -> bool {
        self.session.drawer_open(sidebar, mode)
    }

    /// Dispatch a `navigation` op and let it land.
    fn dispatch_nav(&mut self, op: &str, params: Vec<(&str, Value)>) {
        let services: Arc<dyn BuilderServices> = self.engine.clone();
        services.dispatch_intent(OperationIntent::new(
            "navigation".into(),
            op.to_string(),
            params
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        ));
        self.runtime.block_on(
            self.env
                .wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)),
        );
        self.settle();
    }

    fn main_root(&self) -> Option<EntityUri> {
        self.runtime
            .block_on(self.env.query_sql(ACTIVE_MAIN_ROOT_SQL))
            .expect("read the active Main root")
            .first()
            .and_then(|row| row.get("root_id"))
            .and_then(|v| v.as_string())
            .map(|raw| EntityUri::parse(raw).expect("focus_roots.root_id is a block URI"))
    }

    fn background_tab_id(&self) -> i64 {
        self.runtime
            .block_on(self.env.query_sql(BACKGROUND_TAB_SQL))
            .expect("read a background Main tab")
            .first()
            .and_then(|row| row.get("id"))
            .and_then(|v| v.as_i64())
            .expect("a second open Main tab to close in the background")
    }

    /// Open the sidebar through the same call the toggle handler makes, without
    /// the mouse event that would also move window focus.
    fn open_sidebar(&mut self, sidebar: &str) {
        let services: Arc<dyn BuilderServices> = self.engine.clone();
        services.set_widget_open(sidebar, true);
        self.settle();
        assert!(
            self.is_open(sidebar, DrawerMode::Overlay),
            "{sidebar} must be open, else the claim that follows is vacuous",
        );
    }

    fn element(&self, id: &str) -> ElementInfo {
        self.bounds
            .element_info(id)
            .unwrap_or_else(|| panic!("{id} must be on screen"))
    }

    /// Tap a sidebar's toggle handle — the affordance a user reaches for to
    /// open or close it.
    fn tap_toggle(&self, driver: &SimUserDriver, sidebar: &str) {
        let (x, y) = self.element(&drawer_toggle_id_for(sidebar)).center();
        driver.click_point(x, y);
    }

    /// The row's TEXT element — where a caret-seating click has to land (the
    /// element `SimUserDriver::text_center` aims at).
    fn row_text(&self, row: &str) -> ElementInfo {
        let elements = self.bounds.all_elements();
        ["editable_text", "rendered_text"]
            .into_iter()
            .find_map(|want| {
                elements
                    .iter()
                    .find(|(_, i)| {
                        i.entity_id.as_deref() == Some(row)
                            && i.widget_type.as_ref() == want
                            && i.has_visible_area()
                    })
                    .map(|(_, i)| i.clone())
            })
            .unwrap_or_else(|| panic!("{row} must have a visible text element to tap"))
    }

    /// A point on `row`'s text inside `min_x..max_x` — the exposed page between
    /// the drawers, so the tap lands neither on a drawer nor off the row.
    fn point_on_row_within(&self, row: &str, min_x: f32, max_x: f32) -> (f32, f32) {
        let info = self.row_text(row);
        let lo = info.x.max(min_x);
        let hi = (info.x + info.width).min(max_x);
        assert!(
            hi - lo > 4.0,
            "{row}'s text spans x={}..{}, which leaves nothing inside {min_x}..{max_x} — the tap \
             would land on a drawer instead of on the page beside it",
            info.x,
            info.x + info.width,
        );
        ((lo + hi) / 2.0, info.y + info.height / 2.0)
    }

    /// The dismiss area beside the open overlay drawers, read from the tree
    /// rather than inferred from drawer widths.
    fn scrim(&self) -> ElementInfo {
        self.bounds
            .element_info(holon_frontend::geometry::OVERLAY_SCRIM_ID)
            .expect(
                "an open overlay drawer must offer a dismiss area beside it; the tree draws none, \
                 so the page behind the drawer is inert and the drawer can only be closed by \
                 finding its toggle again",
            )
    }
}

#[test]
fn an_overlay_sidebar_dismisses_on_a_page_tap_and_on_a_tap_beside_it() {
    let mut h = boot("Holon-Overlay-Sidebar-Dismiss", PHONE_WINDOW);
    let driver = h.driver();

    assert_eq!(
        h.mode(LEFT_SIDEBAR),
        DrawerMode::Overlay,
        "in a {PHONE_WINDOW} window the sidebar must float over the page, else neither claim \
         below is about the phone layout",
    );

    // RUNG 1 — the user taps a page in the sidebar.
    h.tap_toggle(&driver, LEFT_SIDEBAR);
    h.settle();
    assert!(
        h.is_open(LEFT_SIDEBAR, DrawerMode::Overlay),
        "the toggle tap must open the sidebar, else the dismissal claim is vacuous",
    );

    let target = EntityUri::block(NAV_TARGET);
    h.navigate_to(&target);
    assert_eq!(
        h.engine.ui_state().focused_block().as_ref(),
        Some(&target),
        "the page tap must actually navigate, else the claim below is vacuous",
    );
    eprintln!(
        "[overlay-dismiss/link] left_open={} focus={:?}",
        h.is_open(LEFT_SIDEBAR, DrawerMode::Overlay),
        h.engine.ui_state().focused_block(),
    );
    assert!(
        !h.is_open(LEFT_SIDEBAR, DrawerMode::Overlay),
        "tapping a page in the overlay sidebar must put it away; it still covers the page the \
         tap just opened, and the page is only reachable by finding the toggle again",
    );

    // RUNG 2 — the user taps the page beside the sidebar. The tap must land on
    // a row that WOULD take the caret, so that "the scrim ate the tap" is a
    // claim about the scrim and not about empty space. The control tap proves
    // the point is live.
    let control = row_uri(ROW_CONTROL);
    let (cx_, cy) = h.point_on_row_within(ROW_CONTROL, 0.0, f32::INFINITY);
    driver.click_point(cx_, cy);
    h.settle();
    assert_eq!(
        h.engine.ui_state().focused_block().as_ref(),
        Some(&control),
        "with the sidebar closed, a tap on {ROW_CONTROL} must focus it — otherwise the rung below \
         proves nothing about the scrim",
    );

    h.tap_toggle(&driver, LEFT_SIDEBAR);
    h.settle();
    assert!(
        h.is_open(LEFT_SIDEBAR, DrawerMode::Overlay),
        "re-opening the sidebar must work, else the outside-tap claim is vacuous",
    );
    let scrim = h.scrim();
    assert!(
        scrim.x > 0.0 && scrim.has_visible_area(),
        "the dismiss area must start past the open drawer and have extent; it is {}..{} — the tap \
         below would not be beside anything",
        scrim.x,
        scrim.x + scrim.width,
    );

    let (x, y) = h.point_on_row_within(ROW_UNDER_SCRIM, scrim.x, scrim.x + scrim.width);
    driver.click_point(x, y);
    h.settle();

    eprintln!(
        "[overlay-dismiss/outside] tap=({x},{y}) over={ROW_UNDER_SCRIM} left_open={} focus={:?}",
        h.is_open(LEFT_SIDEBAR, DrawerMode::Overlay),
        h.engine.ui_state().focused_block(),
    );
    assert!(
        !h.is_open(LEFT_SIDEBAR, DrawerMode::Overlay),
        "tapping the page beside an overlay sidebar must dismiss it",
    );
    assert_eq!(
        h.engine.ui_state().focused_block().as_ref(),
        Some(&control),
        "the dismissing tap belongs to the sidebar, not to the row under it: {ROW_UNDER_SCRIM} \
         must not have taken the caret off {ROW_CONTROL}",
    );

    h.teardown(driver);
}

#[test]
fn a_sidebar_beside_an_overlay_drawer_keeps_its_own_clicks() {
    let mut h = boot("Holon-Midband-Sidebar-Clicks", MID_WINDOW);
    let driver = h.driver();

    assert_eq!(
        h.mode(LEFT_SIDEBAR),
        DrawerMode::Shrink,
        "in a {MID_WINDOW} window the left sidebar must be in flow, else this is not the mid band",
    );
    assert_eq!(
        h.mode(RIGHT_SIDEBAR),
        DrawerMode::Overlay,
        "in a {MID_WINDOW} window the right sidebar must float, else there is no scrim to test",
    );

    h.tap_toggle(&driver, RIGHT_SIDEBAR);
    h.settle();
    assert!(
        h.is_open(RIGHT_SIDEBAR, DrawerMode::Overlay),
        "the right overlay drawer must be open, else nothing draws a scrim",
    );
    assert!(
        h.is_open(LEFT_SIDEBAR, DrawerMode::Shrink),
        "the left sidebar must be open, else its click target is a bare toggle",
    );

    // A click inside the left sidebar belongs to the left sidebar: it is beside
    // the overlay drawer, not under it. Its own toggle is the click target that
    // makes "the click landed" observable.
    h.tap_toggle(&driver, LEFT_SIDEBAR);
    h.settle();
    eprintln!(
        "[overlay-dismiss/midband] left_open={} right_open={}",
        h.is_open(LEFT_SIDEBAR, DrawerMode::Shrink),
        h.is_open(RIGHT_SIDEBAR, DrawerMode::Overlay),
    );
    assert!(
        !h.is_open(LEFT_SIDEBAR, DrawerMode::Shrink),
        "the click inside the left sidebar never reached its toggle — a scrim inset only by the \
         overlay drawer is painted across the in-flow sidebar and swallows its clicks",
    );
    assert!(
        h.is_open(RIGHT_SIDEBAR, DrawerMode::Overlay),
        "a click inside the left sidebar must not dismiss the right overlay drawer",
    );

    // Placing the caret writes the focused block. A drawer keyed on the focus
    // would slam shut while the user is working in the page beside it, so the
    // two legs below separate a caret from a page change.
    let (rx, ry) = h.point_on_row_within(ROW_CONTROL, 0.0, f32::INFINITY);
    h.tap_toggle(&driver, RIGHT_SIDEBAR);
    h.settle();
    assert!(
        !h.is_open(RIGHT_SIDEBAR, DrawerMode::Overlay),
        "close the drawer first so the caret tap reaches the row instead of the scrim",
    );
    let nav_gen_before = h.engine.ui_state().main_view_generation();
    driver.click_point(rx, ry);
    h.settle();
    let focus_after_caret = h.engine.ui_state().focused_block();
    eprintln!(
        "[overlay-dismiss/caret] focus={focus_after_caret:?} nav_gen {nav_gen_before} -> {}",
        h.engine.ui_state().main_view_generation(),
    );
    assert_eq!(
        focus_after_caret.as_ref(),
        Some(&row_uri(ROW_CONTROL)),
        "the caret tap must write the focus, else it cannot stand in for typing",
    );
    assert_eq!(
        h.engine.ui_state().main_view_generation(),
        nav_gen_before,
        "seating a caret moved the main region's view counter — the counter the drawer keys on \
         must mean a page change, or every keystroke reads as navigation",
    );

    // A right-region pin: a focus change the main region never sees.
    let services: Arc<dyn BuilderServices> = h.engine.clone();
    services.set_widget_open(RIGHT_SIDEBAR, true);
    h.settle();
    assert!(
        h.is_open(RIGHT_SIDEBAR, DrawerMode::Overlay),
        "re-open the drawer, else the claim below is vacuous",
    );
    services.dispatch_intent(OperationIntent::new(
        "navigation".into(),
        "focus".into(),
        [
            ("region".to_string(), Value::String("right".to_string())),
            (
                "block_id".to_string(),
                Value::String(ROW_UNDER_SCRIM.to_string()),
            ),
        ]
        .into_iter()
        .collect(),
    ));
    h.runtime.block_on(
        h.env
            .wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)),
    );
    h.settle();
    eprintln!(
        "[overlay-dismiss/pin] focus {focus_after_caret:?} -> {:?} right_open={}",
        h.engine.ui_state().focused_block(),
        h.is_open(RIGHT_SIDEBAR, DrawerMode::Overlay),
    );
    assert_ne!(
        h.engine.ui_state().focused_block(),
        focus_after_caret,
        "the pin must actually move the focus, else the claim below is vacuous",
    );
    assert!(
        h.is_open(RIGHT_SIDEBAR, DrawerMode::Overlay),
        "the focus moved without the main region navigating anywhere — an open sidebar must \
         survive that, or it closes on every caret move and keystroke",
    );

    // The page between the two sidebars is still the overlay drawer's own
    // dismiss area.
    let scrim = h.scrim();
    let (x, y) = h.point_on_row_within(ROW_CONTROL, scrim.x, scrim.x + scrim.width);
    driver.click_point(x, y);
    h.settle();
    eprintln!(
        "[overlay-dismiss/midband-page] tap=({x},{y}) scrim={}..{} right_open={}",
        scrim.x,
        scrim.x + scrim.width,
        h.is_open(RIGHT_SIDEBAR, DrawerMode::Overlay)
    );
    assert!(
        !h.is_open(RIGHT_SIDEBAR, DrawerMode::Overlay),
        "a tap on the page beside the right overlay drawer must still dismiss it",
    );

    h.teardown(driver);
}

/// The counter the auto-close listens to bumps for cursor-moving ops that may
/// not have moved the view at all — `navigation.close` names a ROW, and closing
/// a background one leaves the page exactly where it was.
#[test]
fn only_a_real_page_change_dismisses_an_overlay_sidebar() {
    let mut h = boot("Holon-Nav-Dismiss-Precision", PHONE_WINDOW);
    let driver = h.driver();

    assert_eq!(
        h.mode(LEFT_SIDEBAR),
        DrawerMode::Overlay,
        "in a {PHONE_WINDOW} window the sidebar must float, else this is not the phone layout",
    );

    // A second open tab, so there is a background row to close later.
    h.dispatch_nav(
        "open_tab",
        vec![
            ("region", Value::String("main".into())),
            (
                "block_id",
                Value::String(EntityUri::block(NAV_TARGET).to_string()),
            ),
        ],
    );

    // GOING BACK IS A PAGE CHANGE. It names no target and leaves the focus
    // alone, so only the resolved root says the view moved.
    h.open_sidebar(LEFT_SIDEBAR);
    let root_before = h.main_root();
    h.dispatch_nav("go_back", vec![("region", Value::String("main".into()))]);
    let root_after = h.main_root();
    eprintln!(
        "[overlay-dismiss/go-back] root {root_before:?} -> {root_after:?} left_open={}",
        h.is_open(LEFT_SIDEBAR, DrawerMode::Overlay),
    );
    assert_ne!(
        root_after, root_before,
        "Back must move the Main view, else the claim below is vacuous",
    );
    assert!(
        !h.is_open(LEFT_SIDEBAR, DrawerMode::Overlay),
        "Back put the user on another page — the sidebar covering it must come down, the same as \
         for a page tapped in the sidebar",
    );

    // CLOSING A BACKGROUND TAB IS NOT. The counter bumps because `close` cannot
    // tell whether the cursor moved; the resolved root can.
    // Back consumed the tab it came from, so open another one to leave in the
    // background.
    h.dispatch_nav(
        "open_tab",
        vec![
            ("region", Value::String("main".into())),
            (
                "block_id",
                Value::String(EntityUri::block(NAV_TARGET).to_string()),
            ),
        ],
    );
    h.open_sidebar(LEFT_SIDEBAR);
    let root_before = h.main_root();
    let background = h.background_tab_id();
    h.dispatch_nav("close", vec![("history_id", Value::Integer(background))]);
    let root_after = h.main_root();
    eprintln!(
        "[overlay-dismiss/bg-close] closed={background} root {root_before:?} -> {root_after:?} \
         left_open={}",
        h.is_open(LEFT_SIDEBAR, DrawerMode::Overlay),
    );
    assert_eq!(
        root_after, root_before,
        "closing a BACKGROUND row must leave the Main view where it was, else this leg is not \
         about a spurious bump",
    );
    assert!(
        h.is_open(LEFT_SIDEBAR, DrawerMode::Overlay),
        "the page under the sidebar never changed — dismissing on a background tab closing is \
         the sidebar reacting to something the user cannot see",
    );

    // RE-SELECTING THE PAGE THE VIEW IS ALREADY ON. The op names a target, so
    // it is a deliberate page tap even though the root does not move.
    let current = h.main_root().expect("the Main region is open on a page");
    h.dispatch_nav(
        "focus",
        vec![
            ("region", Value::String("main".into())),
            ("block_id", Value::String(current.to_string())),
        ],
    );
    eprintln!(
        "[overlay-dismiss/reselect] root={current} left_open={}",
        h.is_open(LEFT_SIDEBAR, DrawerMode::Overlay),
    );
    assert!(
        !h.is_open(LEFT_SIDEBAR, DrawerMode::Overlay),
        "tapping a page in the sidebar must put the sidebar away whether or not that page is the \
         one already showing — the gesture is the same and so is what it hides",
    );

    h.teardown(driver);
}

#[test]
fn a_desktop_sidebar_survives_the_same_two_gestures() {
    let mut h = boot("Holon-Shrink-Sidebar-Survives", DESKTOP_WINDOW);
    let driver = h.driver();

    assert_eq!(
        h.mode(LEFT_SIDEBAR),
        DrawerMode::Shrink,
        "in a {DESKTOP_WINDOW} window the sidebar must sit beside the page, else this test is not \
         about the desktop layout",
    );
    assert!(
        h.is_open(LEFT_SIDEBAR, DrawerMode::Shrink),
        "a desktop sidebar starts open, else neither claim below has anything to survive",
    );

    let target = EntityUri::block(NAV_TARGET);
    h.navigate_to(&target);
    assert_eq!(
        h.engine.ui_state().focused_block().as_ref(),
        Some(&target),
        "the navigation must land, else the claim below is vacuous",
    );
    assert!(
        h.is_open(LEFT_SIDEBAR, DrawerMode::Shrink),
        "a desktop sidebar shrinks the page instead of covering it, so navigating must leave it \
         open",
    );

    let (x, y) = h.point_on_row_within(ROW_CONTROL, 0.0, f32::INFINITY);
    driver.click_point(x, y);
    h.settle();

    eprintln!(
        "[overlay-dismiss/desktop] tap=({x},{y}) left_open={} focus={:?}",
        h.is_open(LEFT_SIDEBAR, DrawerMode::Shrink),
        h.engine.ui_state().focused_block(),
    );
    assert!(
        h.is_open(LEFT_SIDEBAR, DrawerMode::Shrink),
        "a click in the desktop page area must not collapse the sidebar",
    );
    assert_eq!(
        h.engine.ui_state().focused_block().as_ref(),
        Some(&row_uri(ROW_CONTROL)),
        "with no scrim in the way the click must reach the row and focus it",
    );

    h.teardown(driver);
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
#[path = "test_init/mod.rs"]
mod test_init;
