//! The top chrome is ONE row, browser-style: the open tabs are reached through
//! a count button in the title row, not through a strip of their own.
//!
//! The budget these rungs hold is how far down the user's first row of content
//! starts. Three stacked bars — title, tab strip, breadcrumb — cost 96px of a
//! phone's ~700px viewport; one row costs 38. The measurement anchors on
//! `live_block` rows, never on a panel container: the sidebar box is pinned to
//! the chrome's bottom edge by construction and would read 38 even with every
//! row inside it pushed down.
//!
//! The element-id scheme is the contract between these rungs and the chrome:
//!
//! | id | what it is |
//! |---|---|
//! | `chrome-tab-count` | title-row button, `displayed_text` = number of open Main tabs, or `▤ !` when the read failed |
//! | `tab-list-row-{history_id}` | one row per open tab, `displayed_text` = its caption |
//! | `tab-list-close-{history_id}` | that row's close affordance |
//! | `tab-list-new` | the new-tab action |
//! | `tab-list-error` | the failure message, when the tabs could not be read |
//!
//! Run: `cargo nextest run -p holon-gpui --test chrome_one_row_windowed
//! --features holon-gpui/pbt`

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use gpui::InputEvent;
use gpui::MouseButton;
use gpui::Pixels;
use gpui::PlatformTextSystem;
use gpui::Point;
use holon_api::EntityUri;
use holon_api::Value;
use holon_frontend::geometry::ElementInfo;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::BuilderServices;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::window_slice::seed::graft_displayed_text_tree;
use holon_integration_tests::test_environment::TestEnvironment;

/// The row `graft_displayed_text_tree` hangs its children under — a second
/// navigation target, so the window can hold two open tabs.
const ZOOM_ROW: &str = "parent";

/// The title-row button that carries the open-tab count and opens the list.
const TAB_COUNT_BUTTON: &str = "chrome-tab-count";
const TAB_LIST_ROW_PREFIX: &str = "tab-list-row-";
const TAB_LIST_CLOSE_PREFIX: &str = "tab-list-close-";
const TAB_LIST_NEW: &str = "tab-list-new";
const TAB_LIST_ERROR: &str = "tab-list-error";
/// What the button must paint when the tabs cannot be resolved — a count is
/// unavailable, not zero.
const TAB_COUNT_ERROR_LABEL: &str = "▤ !";
/// What it must paint while a write has been outstanding too long — the count
/// it holds predates that write.
const TAB_COUNT_WAITING_LABEL: &str = "▤ …";

/// Title-row affordances. Excluded when measuring where CONTENT starts, since
/// they sit inside the chrome rather than below it.
const CHROME_ELEMENT_IDS: &[&str] = &[
    "settings-gear",
    TAB_COUNT_BUTTON,
    TAB_LIST_NEW,
    "tab-strip",
    "breadcrumb-bar",
];

/// The title row is 38px. Content may add its own padding under it, so the
/// one-row claim allows a row plus 24px — still far under the ~96px three bars
/// cost today.
const ONE_ROW_CONTENT_TOP_MAX: f32 = 62.0;

/// Phone-width and desktop-width viewports. The chrome is one row at both:
/// there is no width gate to fall out of sync.
const NARROW_VIEWPORT: (f32, f32) = (390.0, 780.0);
const WIDE_VIEWPORT: (f32, f32) = (1440.0, 900.0);

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

/// Open MAIN tabs as `(history_id, block_id)`, insertion-ordered — the rows the
/// count button counts and the list names. Read from `navigation_history`, not
/// `focus_roots`: the matview omits the NULL-block rows that blank tabs are, so
/// it cannot see a tab the new-tab action just created.
fn open_main_tabs(
    env: &TestEnvironment,
    runtime: &tokio::runtime::Runtime,
) -> Vec<(i64, Option<EntityUri>)> {
    runtime
        .block_on(env.query_sql(
            "SELECT id, block_id FROM navigation_history WHERE region = 'main' AND closed_at IS \
             NULL ORDER BY id",
        ))
        .expect("read the open Main tabs")
        .iter()
        .map(|row| {
            let id = row
                .get("id")
                .and_then(|v| v.as_i64())
                .expect("navigation_history.id is an integer");
            let block = row
                .get("block_id")
                .and_then(|v| v.as_string())
                .map(|raw| EntityUri::parse(raw).expect("block_id is a block URI"));
            (id, block)
        })
        .collect()
}

/// The `navigation_history` row the Main cursor points at.
fn cursor_history_id(env: &TestEnvironment, runtime: &tokio::runtime::Runtime) -> Option<i64> {
    runtime
        .block_on(env.query_sql("SELECT history_id FROM navigation_cursor WHERE region = 'main'"))
        .expect("read the Main cursor")
        .first()
        .and_then(|r| r.get("history_id"))
        .and_then(|v| v.as_i64())
}

/// The page the Main region is showing, or `None` when the active tab is blank
/// (no `focus_roots` row) and the panel falls through to its default render.
fn main_view_root(env: &TestEnvironment, runtime: &tokio::runtime::Runtime) -> Option<EntityUri> {
    runtime
        .block_on(env.query_sql(ACTIVE_MAIN_ROOT_SQL))
        .expect("read the active Main root")
        .first()
        .and_then(|r| r.get("root_id"))
        .and_then(|v| v.as_string())
        .map(|raw| EntityUri::parse(raw).expect("focus_roots.root_id is a block URI"))
}

fn active_main_root(env: &TestEnvironment, runtime: &tokio::runtime::Runtime) -> EntityUri {
    let rows = runtime
        .block_on(env.query_sql(ACTIVE_MAIN_ROOT_SQL))
        .expect("read the active Main root");
    let raw = rows
        .first()
        .and_then(|r| r.get("root_id"))
        .and_then(|v| v.as_string())
        .expect("the Main region must have an active root")
        .to_string();
    EntityUri::parse(&raw).expect("focus_roots.root_id is a block URI")
}

/// Where the first painted OUTLINE ROW lands, in logical px from the window
/// top — i.e. how much vertical space the chrome takes from the user's content.
///
/// Anchored on `live_block` rows specifically, not on the topmost tracked
/// element of any kind: the panel CONTAINERS (`drawer_toggle::…`, the
/// full-height sidebar box) are pinned to the chrome's bottom edge by
/// construction and would report 38 even if every row inside them were pushed
/// down. The rows are what the user reads.
fn content_top(bounds: &BoundsRegistry) -> Option<(String, f32)> {
    bounds
        .all_elements()
        .into_iter()
        .filter(|(_, info)| info.height > 0.0 && info.widget_type.as_ref() == "live_block")
        .map(|(id, info)| (id, info.y))
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// Settle until the count button paints `expected`, and return what it ended up
/// showing. The tabs are read asynchronously, so the button reaches the truth a
/// frame or two after the database does; a count that NEVER gets there is the
/// regression worth failing on, and the caller's assertion says so.
fn count_settles_to(w: &mut TwoTabWindow, expected: &str) -> Option<String> {
    let mut seen = count_button_text(&w.bounds);
    for _ in 0..10 {
        if seen.as_deref() == Some(expected) {
            break;
        }
        settle_to_fixed_point(&mut w.app, &w.bounds, &w.runtime, Duration::from_secs(120));
        seen = count_button_text(&w.bounds);
    }
    seen
}

/// What the title row's count button is currently painting.
fn count_button_text(bounds: &BoundsRegistry) -> Option<String> {
    bounds
        .element_info(TAB_COUNT_BUTTON)?
        .displayed_text
        .as_ref()
        .map(|t| t.to_string())
}

/// The `tab-list-row-{history_id}` rows the popup is drawing, by history id.
fn tab_list_rows(bounds: &BoundsRegistry) -> Vec<(i64, ElementInfo)> {
    let mut rows: Vec<(i64, ElementInfo)> = bounds
        .all_elements()
        .into_iter()
        .filter_map(|(id, info)| {
            let rest = id.strip_prefix(TAB_LIST_ROW_PREFIX)?;
            rest.parse::<i64>().ok().map(|hid| (hid, info))
        })
        .collect();
    rows.sort_by_key(|(hid, _)| *hid);
    rows
}

fn center_of(info: &ElementInfo) -> Point<Pixels> {
    let (x, y) = info.center();
    Point {
        x: Pixels::from(x),
        y: Pixels::from(y),
    }
}

/// Dispatch a real left click at `center`.
fn click_at(
    app: &mut HeadlessAppContext,
    window: gpui::AnyWindowHandle,
    center: Point<Pixels>,
    what: &str,
) {
    app.update(|cx| {
        window
            .update(cx, |_, win, cx| {
                win.dispatch_event(
                    gpui::MouseMoveEvent {
                        position: center,
                        pressed_button: None,
                        modifiers: Default::default(),
                    }
                    .to_platform_input(),
                    cx,
                );
                win.dispatch_event(
                    gpui::MouseDownEvent {
                        position: center,
                        button: MouseButton::Left,
                        modifiers: Default::default(),
                        click_count: 1,
                        first_mouse: false,
                    }
                    .to_platform_input(),
                    cx,
                );
                win.dispatch_event(
                    gpui::MouseUpEvent {
                        position: center,
                        button: MouseButton::Left,
                        modifiers: Default::default(),
                        click_count: 1,
                    }
                    .to_platform_input(),
                    cx,
                );
            })
            .unwrap_or_else(|e| panic!("window alive for the {what} click: {e}"));
    });
}

/// A census of every tracked element with its top edge — the evidence a red log
/// needs to tell "the chrome shrank" from "content stopped painting".
fn top_edges_report(bounds: &BoundsRegistry) -> String {
    let mut rows: Vec<(f32, String, f32)> = bounds
        .all_elements()
        .into_iter()
        .map(|(id, info)| (info.y, id, info.height))
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.truncate(12);
    rows.iter()
        .map(|(y, id, h)| format!("  y={y:.1} h={h:.1} {id}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Boot a window over a fresh environment with a page grafted in and a second
/// Main tab open, so the chrome has something to count.
struct TwoTabWindow {
    app: HeadlessAppContext,
    runtime: Arc<tokio::runtime::Runtime>,
    env: TestEnvironment,
    engine: Arc<holon_frontend::reactive::ReactiveEngine>,
    bounds: BoundsRegistry,
    rebind: holon_gpui::RebindHandle,
}

fn boot_two_tab_window(title: &str) -> TwoTabWindow {
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
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    runtime
        .block_on(graft_displayed_text_tree(&env))
        .expect("graft a page and navigate Main onto it");
    runtime
        .block_on(env.wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)));
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(120));

    let services: Arc<dyn BuilderServices> = engine.clone();
    dispatch(
        &services,
        "open_tab",
        vec![
            ("region", Value::String("main".into())),
            (
                "block_id",
                Value::String(EntityUri::block(ZOOM_ROW).to_string()),
            ),
        ],
    );
    runtime
        .block_on(env.wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)));
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(120));

    TwoTabWindow {
        app,
        runtime,
        env,
        engine,
        bounds,
        rebind,
    }
}

/// Re-seed the viewport the chrome lays out against and force a repaint.
fn set_viewport(w: &mut TwoTabWindow, (width, height): (f32, f32)) {
    w.engine
        .ui_state()
        .set_viewport(holon_frontend::reactive::ViewportInfo {
            width_px: width,
            height_px: height,
            scale_factor: 1.0,
        });
    let rebind = &w.rebind;
    w.app.update(|cx| rebind.set_safe_area_bottom(0.0, cx));
    settle_to_fixed_point(&mut w.app, &w.bounds, &w.runtime, Duration::from_secs(120));
}

fn shutdown(w: TwoTabWindow) {
    let TwoTabWindow {
        mut app,
        env,
        rebind,
        ..
    } = w;
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(env);
}

#[test]
fn the_chrome_is_one_row_at_every_width() {
    let mut w = boot_two_tab_window("Holon-Chrome-One-Row");

    let tabs = open_main_tabs(&w.env, &w.runtime);
    assert!(
        tabs.len() >= 2,
        "the fixture must leave at least two Main tabs open, else a chrome that hides the tab \
         strip when empty would pass this test without collapsing anything; open tabs are {tabs:?}",
    );

    let mut measured: Vec<(&str, f32, String, Option<String>)> = Vec::new();
    for (label, viewport) in [("narrow", NARROW_VIEWPORT), ("wide", WIDE_VIEWPORT)] {
        set_viewport(&mut w, viewport);
        let (top_id, top) = content_top(&w.bounds).unwrap_or_else(|| {
            panic!(
                "no content element was painted at {label} width, so there is nothing to measure \
                 the chrome against:\n{}",
                top_edges_report(&w.bounds),
            )
        });
        let button = w
            .bounds
            .element_info(TAB_COUNT_BUTTON)
            .map(|info| format!("y={:.1} text={:?}", info.y, info.displayed_text));
        measured.push((label, top, top_id, button));
    }

    let count_button = w.bounds.element_info(TAB_COUNT_BUTTON);
    let gear = w.bounds.element_info("settings-gear");
    let census = top_edges_report(&w.bounds);

    shutdown(w);

    for (label, top, top_id, button) in &measured {
        eprintln!(
            "[chrome-one-row/{label}] content_top={top:.1} ({top_id}) tab_count_button={button:?}"
        );
    }
    eprintln!("[chrome-one-row] topmost tracked elements:\n{census}");

    for (label, top, top_id, _) in &measured {
        assert!(
            *top <= ONE_ROW_CONTENT_TOP_MAX,
            "at {label} width the first content ({top_id}) starts {top:.1}px down, so the chrome \
             is still {top:.1}px tall. One title row is {ONE_ROW_CONTENT_TOP_MAX}px including \
             content padding; the tab strip and the breadcrumb bar must live IN the title row, \
             not as rows of their own.",
        );
    }

    let button = count_button.unwrap_or_else(|| {
        panic!(
            "the title row draws no {TAB_COUNT_BUTTON:?} button, so there is no way to reach the \
             open tabs once the strip is gone:\n{census}"
        )
    });
    let gear = gear.expect("the toolbar gear is tracked, so the title row's own y is known");
    assert!(
        (button.y - gear.y).abs() <= 8.0,
        "the tab-count button sits at y={:.1} and the toolbar gear at y={:.1}: the button must \
         share the title row with the rest of the toolbar, not open a second row",
        button.y,
        gear.y,
    );

    let expected = format!("{}", tabs.len());
    let shown = button
        .displayed_text
        .as_ref()
        .map(|t| t.to_string())
        .unwrap_or_default();
    assert!(
        shown.contains(&expected),
        "{} Main tabs are open but the count button shows {shown:?}. The count is the only thing \
         left telling the user how many tabs they have.",
        tabs.len(),
    );
}

#[test]
fn the_tab_count_button_opens_a_list_that_switches_and_creates_tabs() {
    let mut w = boot_two_tab_window("Holon-Chrome-Tab-List");

    let tabs = open_main_tabs(&w.env, &w.runtime);
    assert!(
        tabs.len() >= 2,
        "the fixture must leave at least two Main tabs open, else switching between them proves \
         nothing; open tabs are {tabs:?}",
    );

    let button = w.bounds.element_info(TAB_COUNT_BUTTON);
    let census = top_edges_report(&w.bounds);
    let Some(button) = button else {
        shutdown(w);
        panic!(
            "the title row draws no {TAB_COUNT_BUTTON:?} button, so the tab list has no door:\n\
             {census}"
        );
    };

    let window = w.rebind.window();
    click_at(&mut w.app, window, center_of(&button), "tab-count button");
    settle_to_fixed_point(&mut w.app, &w.bounds, &w.runtime, Duration::from_secs(120));

    let rows = tab_list_rows(&w.bounds);
    let missing_close: Vec<i64> = rows
        .iter()
        .map(|(hid, _)| *hid)
        .filter(|hid| {
            w.bounds
                .element_info(&format!("{TAB_LIST_CLOSE_PREFIX}{hid}"))
                .is_none()
        })
        .collect();
    let new_button = w.bounds.element_info(TAB_LIST_NEW);
    let labels: Vec<(i64, Option<String>)> = rows
        .iter()
        .map(|(hid, info)| (*hid, info.displayed_text.as_ref().map(|t| t.to_string())))
        .collect();
    eprintln!("[chrome-tab-list/open] open_tabs={tabs:?} list_rows={labels:?}");

    let open_ids: Vec<i64> = tabs.iter().map(|(hid, _)| *hid).collect();
    let row_ids: Vec<i64> = rows.iter().map(|(hid, _)| *hid).collect();
    if row_ids != open_ids || !missing_close.is_empty() || new_button.is_none() {
        let census = top_edges_report(&w.bounds);
        shutdown(w);
        panic!(
            "tapping the tab-count button must open a list naming every open tab, each with its \
             own close affordance, plus a new-tab action. Open tabs are {open_ids:?}; the list \
             drew rows {row_ids:?} (labels {labels:?}), rows without a \
             {TAB_LIST_CLOSE_PREFIX}* affordance are {missing_close:?}, and {TAB_LIST_NEW:?} is \
             {}.\n{census}",
            if new_button.is_some() {
                "present"
            } else {
                "absent"
            },
        );
    }
    for (hid, label) in &labels {
        assert!(
            label.as_ref().is_some_and(|t| !t.trim().is_empty()),
            "the list row for tab {hid} shows {label:?} — a row the user cannot read names no tab",
        );
    }

    // SWITCH. Pick a tab on a DIFFERENT page than the cursor's, so landing on it
    // moves the view root the assertion reads.
    let current = active_main_root(&w.env, &w.runtime);
    let (target_id, target_root) = tabs
        .iter()
        .filter_map(|(hid, block)| block.clone().map(|b| (*hid, b)))
        .find(|(_, root)| root != &current)
        .expect("a second open tab whose page differs from the active one");
    let target_row = rows
        .iter()
        .find(|(hid, _)| *hid == target_id)
        .map(|(_, info)| center_of(info))
        .expect("the list draws a row for the tab being switched to");
    click_at(&mut w.app, window, target_row, "tab list row");
    w.runtime.block_on(
        w.env
            .wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)),
    );
    settle_to_fixed_point(&mut w.app, &w.bounds, &w.runtime, Duration::from_secs(120));

    let switched_root = active_main_root(&w.env, &w.runtime);
    eprintln!(
        "[chrome-tab-list/switch] target={target_id} ({target_root}) root={current}->\
         {switched_root}"
    );
    assert_eq!(
        switched_root, target_root,
        "choosing tab {target_id} in the list must move the Main view onto {target_root}; it is \
         on {switched_root}",
    );
    assert!(
        tab_list_rows(&w.bounds).is_empty(),
        "the list must close once it has switched tabs — a popup standing over the page the user \
         just navigated to is exactly the stale chrome this feature removes; it still draws {:?}",
        tab_list_rows(&w.bounds)
            .iter()
            .map(|(hid, _)| *hid)
            .collect::<Vec<_>>(),
    );
    let count_after_switch = count_settles_to(&mut w, "▤ 2");
    assert_eq!(
        count_after_switch.as_deref(),
        Some("▤ 2"),
        "switching tabs opens and closes nothing, so the count must still read the two open tabs; \
         it reads {count_after_switch:?}",
    );

    // CLOSE. The list's own close control must actually close the tab, not just
    // exist — and the count in the title row must follow it down.
    let button = w
        .bounds
        .element_info(TAB_COUNT_BUTTON)
        .expect("the count button survives a tab switch");
    click_at(&mut w.app, window, center_of(&button), "tab-count button");
    settle_to_fixed_point(&mut w.app, &w.bounds, &w.runtime, Duration::from_secs(120));

    let doomed = tabs
        .iter()
        .map(|(hid, _)| *hid)
        .find(|hid| *hid != target_id)
        .expect("a tab other than the one just switched to");
    let close_control = w
        .bounds
        .element_info(&format!("{TAB_LIST_CLOSE_PREFIX}{doomed}"))
        .unwrap_or_else(|| panic!("the list draws a close control for tab {doomed}"));
    click_at(
        &mut w.app,
        window,
        center_of(&close_control),
        "tab list close",
    );
    w.runtime.block_on(
        w.env
            .wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)),
    );
    settle_to_fixed_point(&mut w.app, &w.bounds, &w.runtime, Duration::from_secs(120));

    let open_after_close = open_main_tabs(&w.env, &w.runtime);
    let cursor_after_close = cursor_history_id(&w.env, &w.runtime);
    let count_after_close = count_settles_to(&mut w, "▤ 1");
    eprintln!(
        "[chrome-tab-list/close] closed={doomed} open={:?} cursor={cursor_after_close:?} \
         count={count_after_close:?}",
        open_after_close.iter().map(|(h, _)| *h).collect::<Vec<_>>(),
    );
    assert!(
        !open_after_close.iter().any(|(hid, _)| *hid == doomed),
        "clicking the close control must close tab {doomed}; the open tabs are still {:?}",
        open_after_close.iter().map(|(h, _)| *h).collect::<Vec<_>>(),
    );
    assert_eq!(
        count_after_close.as_deref(),
        Some("▤ 1"),
        "one of the two tabs was closed, so the title row must now count one; it reads \
         {count_after_close:?}. A count that lags the tabs is the stale-chrome failure the \
         view-generation latch exists to prevent.",
    );
    assert_eq!(
        cursor_after_close,
        Some(target_id),
        "closing a tab the cursor was NOT on must leave the cursor where it was ({target_id}); it \
         moved to {cursor_after_close:?}",
    );

    // NEW TAB. Closing a tab leaves the list STANDING — closing several in a row
    // is the point of a list — so the new-tab action is still on screen.
    let new_button = w
        .bounds
        .element_info(TAB_LIST_NEW)
        .expect("closing a tab keeps the list open, so its new-tab action is still drawn");
    click_at(&mut w.app, window, center_of(&new_button), "new tab");
    w.runtime.block_on(
        w.env
            .wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)),
    );
    settle_to_fixed_point(&mut w.app, &w.bounds, &w.runtime, Duration::from_secs(120));

    let list_after_new: Vec<i64> = tab_list_rows(&w.bounds)
        .iter()
        .map(|(hid, _)| *hid)
        .collect();
    let after = open_main_tabs(&w.env, &w.runtime);
    let cursor_after = cursor_history_id(&w.env, &w.runtime);
    let view_root_after = main_view_root(&w.env, &w.runtime);
    let count_after_new = count_settles_to(&mut w, "▤ 2");
    let newest = after.iter().map(|(hid, _)| *hid).max();
    let newest_block = after
        .iter()
        .find(|(hid, _)| Some(*hid) == newest)
        .and_then(|(_, block)| block.clone());
    eprintln!(
        "[chrome-tab-list/new] tabs {}->{} newest={newest:?} cursor={cursor_after:?} \
         view_root={view_root_after:?} count={count_after_new:?}",
        open_after_close.len(),
        after.len(),
    );

    shutdown(w);

    assert_eq!(
        after.len(),
        open_after_close.len() + 1,
        "the new-tab action must open one more tab: {} were open, {} are now ({after:?})",
        open_after_close.len(),
        after.len(),
    );
    assert_eq!(
        count_after_new.as_deref(),
        Some("▤ 2"),
        "the new tab brings the count back to two; the title row reads {count_after_new:?}",
    );
    assert!(
        list_after_new.is_empty(),
        "the list must close once it has created a tab; it still draws {list_after_new:?}",
    );
    assert_eq!(
        cursor_after, newest,
        "a new tab the user cannot see is not a new tab — the cursor must land on the tab that was \
         just created ({newest:?}), it sits on {cursor_after:?}",
    );
    assert_eq!(
        newest_block, None,
        "a new tab is blank: it names no page, so the panel shows the region's default view \
         instead of repeating the page the user was on; it names {newest_block:?}",
    );
    assert_eq!(
        view_root_after, None,
        "with the cursor on a blank tab the Main region has no view root — a root here means the \
         new tab is showing some other tab's page",
    );
}

/// A resolution that FAILS must say so in the chrome. `▤ 0` is the one thing it
/// must never paint: with the strip gone the count is all the user has, and a
/// zero reads as "no tabs are open" rather than "I could not find out".
#[test]
fn the_count_button_shows_the_failure_not_a_plausible_zero() {
    let mut w = boot_two_tab_window("Holon-Chrome-Tab-Error");

    let before = count_button_text(&w.bounds);
    assert_eq!(
        before.as_deref(),
        Some("▤ 2"),
        "the button must be showing a real count before the fault, else this proves nothing",
    );

    // Fault injected at the seam's own parse boundary: an open row whose
    // block_id is not a URI makes `region_open_tabs` return Err, the way a
    // schema skew or a no-Turso wiring would.
    w.runtime
        .block_on(w.env.query_sql(
            "INSERT INTO navigation_history (region, block_id) VALUES ('main', 'not a block uri')",
        ))
        .expect("inject an unparseable open tab row");
    // Re-read until the chrome notices. `activate` bumps the main view
    // generation, the wake-up the chrome's latch watches — the same one a real
    // tab switch gives it. Repeated because a raw INSERT is not itself an event
    // the app hears, so an early re-read can still precede the row.
    let services: Arc<dyn BuilderServices> = w.engine.clone();
    let cursor = cursor_history_id(&w.env, &w.runtime).expect("a Main cursor to re-activate");
    // Waits on the STATE (a failed read clears the tabs), not on the button, so
    // the assertion below is a real claim about what the title row paints
    // rather than a restatement of the loop's own exit condition.
    let mut after = count_button_text(&w.bounds);
    for _ in 0..25 {
        let rebind = &w.rebind;
        if w.app.update(|cx| rebind.drawn_tab_count(cx)) == Some(0) {
            break;
        }
        dispatch(
            &services,
            "activate",
            vec![
                ("region", Value::String("main".into())),
                ("history_id", Value::Integer(cursor)),
            ],
        );
        w.runtime.block_on(
            w.env
                .wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)),
        );
        // An engine-side generation bump notifies nothing by itself, so the
        // window would never re-render and never re-read. A real gesture always
        // carries a notify; stand in for one.
        let rebind = &w.rebind;
        w.app.update(|cx| rebind.set_safe_area_bottom(0.0, cx));
        settle_to_fixed_point(&mut w.app, &w.bounds, &w.runtime, Duration::from_secs(120));
    }
    after = count_button_text(&w.bounds);
    let rebind = &w.rebind;
    let state_tabs = w.app.update(|cx| rebind.drawn_tab_count(cx));
    eprintln!(
        "[chrome-tab-error] button before={before:?} after={after:?} state_tabs={state_tabs:?}"
    );

    // Open the list: the message itself lives there.
    let button = w
        .bounds
        .element_info(TAB_COUNT_BUTTON)
        .expect("the count button is still drawn while the read is failing");
    let window = w.rebind.window();
    click_at(&mut w.app, window, center_of(&button), "tab-count button");
    settle_to_fixed_point(&mut w.app, &w.bounds, &w.runtime, Duration::from_secs(120));

    let error_panel = w
        .bounds
        .element_info(TAB_LIST_ERROR)
        .and_then(|info| info.displayed_text.as_ref().map(|t| t.to_string()));
    let rows_drawn: Vec<i64> = tab_list_rows(&w.bounds)
        .iter()
        .map(|(hid, _)| *hid)
        .collect();
    eprintln!("[chrome-tab-error] list error={error_panel:?} rows={rows_drawn:?}");

    // RECOVERY. Remove the bad row: the next read succeeds, and the chrome must
    // come back rather than staying stuck on the failure.
    w.runtime
        .block_on(
            w.env
                .query_sql("DELETE FROM navigation_history WHERE block_id = 'not a block uri'"),
        )
        .expect("remove the injected row");
    let mut recovered = count_button_text(&w.bounds);
    for _ in 0..25 {
        if recovered.as_deref() == Some("▤ 2") {
            break;
        }
        dispatch(
            &services,
            "activate",
            vec![
                ("region", Value::String("main".into())),
                ("history_id", Value::Integer(cursor)),
            ],
        );
        w.runtime.block_on(
            w.env
                .wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)),
        );
        let rebind = &w.rebind;
        w.app.update(|cx| rebind.set_safe_area_bottom(0.0, cx));
        settle_to_fixed_point(&mut w.app, &w.bounds, &w.runtime, Duration::from_secs(120));
        recovered = count_button_text(&w.bounds);
    }
    eprintln!("[chrome-tab-error] recovered={recovered:?}");

    shutdown(w);

    assert_eq!(
        after.as_deref(),
        Some(TAB_COUNT_ERROR_LABEL),
        "with the tab read failing the button must paint {TAB_COUNT_ERROR_LABEL:?}; it paints \
         {after:?}. A number here is a fabrication — the app does not know how many tabs are \
         open, and {:?} in particular reads as 'you have no tabs'.",
        "▤ 0",
    );
    assert!(
        error_panel.is_some_and(|msg| msg.contains("Tabs unavailable")),
        "opening the list on a failed read must show the reason, not an empty list",
    );
    assert!(
        rows_drawn.is_empty(),
        "a failed read has no tabs to list; it drew rows {rows_drawn:?}",
    );
    assert_eq!(
        recovered.as_deref(),
        Some("▤ 2"),
        "once the bad row is gone the next read succeeds, so the chrome must show the tabs again \
         rather than staying stuck on the failure; it reads {recovered:?}",
    );
}

/// Under a notch the list must still hang directly under the one chrome row.
///
/// The popup is an absolutely-positioned child of the page, and the page pads
/// itself by the top inset — so an anchor that adds the inset again puts the
/// list a notch-height too low on exactly the devices that have one.
#[test]
fn the_tab_list_hangs_under_the_title_row_beneath_a_notch() {
    let mut w = boot_two_tab_window("Holon-Chrome-Tab-List-Notch");

    const NOTCH: f32 = 40.0;
    let rebind = &w.rebind;
    w.app.update(|cx| rebind.set_safe_area_top(NOTCH, cx));
    settle_to_fixed_point(&mut w.app, &w.bounds, &w.runtime, Duration::from_secs(120));

    let button = w
        .bounds
        .element_info(TAB_COUNT_BUTTON)
        .expect("the count button is drawn under a notch");
    let button_y = button.y;
    let window = w.rebind.window();
    click_at(&mut w.app, window, center_of(&button), "tab-count button");
    settle_to_fixed_point(&mut w.app, &w.bounds, &w.runtime, Duration::from_secs(120));

    let rows = tab_list_rows(&w.bounds);
    let first_row_y = rows.first().map(|(_, info)| info.y);
    eprintln!(
        "[chrome-tab-list/notch] safe_area_top={NOTCH} button_y={button_y:.1} \
         first_row_y={first_row_y:?}"
    );

    shutdown(w);

    let first_row_y = first_row_y.expect("the list draws its rows under a notch");
    // The window is bounded on BOTH sides on purpose. Too low means the inset
    // got counted twice — once by the page's padding, again by the anchor — and
    // the list floats a notch-height below the chrome. Too high means the
    // anchor forgot the inset and the list rides up over the title row it is
    // supposed to hang from.
    let chrome_bottom = NOTCH + 38.0;
    let ceiling = chrome_bottom + 24.0;
    assert!(
        first_row_y >= chrome_bottom && first_row_y <= ceiling,
        "with a {NOTCH}px top inset the list's first row sits at y={first_row_y:.1}; it must hang \
         between the chrome row's bottom edge (y={chrome_bottom:.1}, the button is at \
         y={button_y:.1}) and y={ceiling:.1}.",
    );
}

/// Closing the tab you are LOOKING AT is the ordinary gesture: the count must
/// drop and the highlight must land on the survivor.
#[test]
fn closing_the_active_tab_counts_down_and_moves_the_highlight() {
    let mut w = boot_two_tab_window("Holon-Chrome-Close-Active");

    let tabs = open_main_tabs(&w.env, &w.runtime);
    assert!(tabs.len() >= 2, "need two tabs to close one; got {tabs:?}");
    let active = cursor_history_id(&w.env, &w.runtime).expect("a Main cursor");
    let survivor = tabs
        .iter()
        .map(|(hid, _)| *hid)
        .find(|hid| *hid != active)
        .expect("a tab other than the active one");

    let button = w
        .bounds
        .element_info(TAB_COUNT_BUTTON)
        .expect("the count button is drawn");
    let window = w.rebind.window();
    click_at(&mut w.app, window, center_of(&button), "tab-count button");
    settle_to_fixed_point(&mut w.app, &w.bounds, &w.runtime, Duration::from_secs(120));

    // Close the ACTIVE tab, not a background one.
    let close_control = w
        .bounds
        .element_info(&format!("{TAB_LIST_CLOSE_PREFIX}{active}"))
        .unwrap_or_else(|| panic!("the list draws a close control for the active tab {active}"));
    click_at(
        &mut w.app,
        window,
        center_of(&close_control),
        "close active tab",
    );
    w.runtime.block_on(
        w.env
            .wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)),
    );
    let count = count_settles_to(&mut w, "▤ 1");
    let rebind = &w.rebind;
    let highlighted = w.app.update(|cx| rebind.drawn_active_tab(cx));
    let open_after = open_main_tabs(&w.env, &w.runtime);
    let cursor_after = cursor_history_id(&w.env, &w.runtime);
    eprintln!(
        "[chrome-close-active] closed={active} survivor={survivor} count={count:?} \
         highlighted={highlighted:?} open={:?} cursor={cursor_after:?}",
        open_after.iter().map(|(h, _)| *h).collect::<Vec<_>>(),
    );

    shutdown(w);

    assert!(
        !open_after.iter().any(|(hid, _)| *hid == active),
        "the active tab {active} must actually close; open tabs are {open_after:?}",
    );
    assert_eq!(
        count.as_deref(),
        Some("▤ 1"),
        "one tab is left, so the title row must count one; it reads {count:?}. Showing two — or \
         re-highlighting the tab that was just closed — is the pre-op read winning over what the \
         user did.",
    );
    assert_eq!(
        highlighted,
        Some(survivor),
        "the highlight must move to the surviving tab {survivor}; it sits on {highlighted:?}",
    );
    assert_eq!(
        cursor_after,
        Some(survivor),
        "the engine's cursor must follow to {survivor} too; it is on {cursor_after:?}",
    );
}

/// An activate that changes nothing must cost ONE re-read, not a storm.
///
/// A no-op is indistinguishable from a not-yet-landed write to anything that
/// compares before-and-after state, so a retry loop built on that comparison
/// re-reads until it gives up — invisibly, because the screen is right the
/// whole time.
#[test]
fn a_no_op_tab_switch_costs_one_read() {
    let mut w = boot_two_tab_window("Holon-Chrome-No-Op-Read");

    let active = cursor_history_id(&w.env, &w.runtime).expect("a Main cursor");
    let rebind = &w.rebind;
    let before = w
        .app
        .update(|cx| rebind.tab_reads_issued(cx))
        .expect("the window built a root view");

    // Jump to the tab that is ALREADY active, through the chrome's own handler
    // — the path `cmd-N` takes. Dispatching the op directly would skip the very
    // code that decides whether to re-read.
    let nth = open_main_tabs(&w.env, &w.runtime)
        .iter()
        .position(|(hid, _)| *hid == active)
        .expect("the active tab is among the open tabs")
        + 1;
    let rebind = &w.rebind;
    w.app.update(|cx| rebind.jump_to_tab(nth, cx));
    w.runtime.block_on(
        w.env
            .wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)),
    );
    for _ in 0..6 {
        let rebind = &w.rebind;
        w.app.update(|cx| rebind.set_safe_area_bottom(0.0, cx));
        settle_to_fixed_point(&mut w.app, &w.bounds, &w.runtime, Duration::from_secs(120));
    }

    let rebind = &w.rebind;
    let after = w
        .app
        .update(|cx| rebind.tab_reads_issued(cx))
        .expect("the window built a root view");
    let count = count_button_text(&w.bounds);
    eprintln!("[chrome-noop-reads] reads {before} -> {after} count={count:?}");

    shutdown(w);

    let spent = after - before;
    assert!(
        spent <= 2,
        "a no-op switch cost {spent} tab reads ({before} -> {after}). Each read is two SQL \
         queries, and nothing on screen changes while they run, so a retry loop here is pure \
         waste on every keypress that lands on the tab you are already on.",
    );
    assert_eq!(
        count.as_deref(),
        Some("▤ 2"),
        "the count must still be right after a no-op; it reads {count:?}",
    );
}

/// A REFUSED write must keep saying so, and say it in the log too.
///
/// The re-read that follows a successful write clears the error as part of
/// asking for fresh data, so scheduling one after a refusal would wipe the
/// message a frame after it appeared and leave a confident-looking count in its
/// place.
#[test]
fn a_refused_tab_write_keeps_saying_so() {
    test_init::begin_case();
    let mut w = boot_two_tab_window("Holon-Chrome-Refused-Write");

    let before = count_button_text(&w.bounds);
    assert_eq!(
        before.as_deref(),
        Some("▤ 2"),
        "the button must show a real count before the refusal, else this proves nothing",
    );

    // Activate a history row that does not exist: the engine refuses, so the
    // write completes with an Err rather than never completing.
    let missing = open_main_tabs(&w.env, &w.runtime)
        .iter()
        .map(|(hid, _)| *hid)
        .max()
        .expect("an open tab")
        + 9_000;
    let rebind = &w.rebind;
    w.app.update(|cx| rebind.activate_tab_for_test(missing, cx));

    let mut frames = Vec::new();
    for _ in 0..6 {
        settle_to_fixed_point(&mut w.app, &w.bounds, &w.runtime, Duration::from_secs(120));
        frames.push(count_button_text(&w.bounds));
    }
    let logged: Vec<String> = test_init::captured_problems()
        .into_iter()
        .map(|p| p.message)
        .filter(|m| m.contains("tab operation refused"))
        .collect();
    eprintln!("[chrome-refused-write] target={missing} frames={frames:?} logged={logged:?}");

    // A write that DOES land answers the refusal: the error is the last write's
    // verdict, not a permanent state the user has to restart out of.
    let survivor = open_main_tabs(&w.env, &w.runtime)
        .iter()
        .map(|(hid, _)| *hid)
        .next()
        .expect("an open tab to switch to");
    let rebind = &w.rebind;
    w.app
        .update(|cx| rebind.activate_tab_for_test(survivor, cx));
    w.runtime.block_on(
        w.env
            .wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)),
    );
    let cleared = count_settles_to(&mut w, "▤ 2");
    eprintln!("[chrome-refused-write] after a good write: {cleared:?}");

    shutdown(w);

    let error_frames = frames
        .iter()
        .filter(|f| f.as_deref() == Some(TAB_COUNT_ERROR_LABEL))
        .count();
    assert!(
        error_frames >= 2,
        "a refused write must keep saying so: the button showed {TAB_COUNT_ERROR_LABEL:?} in \
         {error_frames} of {} sampled frames ({frames:?}). Flashing the error for one frame and \
         then painting a count reads as 'it worked'.",
        frames.len(),
    );
    assert!(
        !logged.is_empty(),
        "the refusal must reach the log as well as the screen — nothing matching 'tab operation \
         refused' was captured",
    );
    assert_eq!(
        cleared.as_deref(),
        Some("▤ 2"),
        "a write that lands must answer the refusal — the message belongs to the last write, not \
         to the session; the button still reads {cleared:?}",
    );
}

/// A write that never reports back must say the count is waiting on it, rather
/// than letting a pre-op number pass for current.
#[test]
fn a_write_that_never_lands_says_the_count_is_waiting() {
    test_init::begin_case();
    let mut w = boot_two_tab_window("Holon-Chrome-Hung-Write");

    let rebind = &w.rebind;
    w.app.update(|cx| rebind.begin_stuck_tab_write_for_test(cx));

    // Past the disclosure budget the button must stop presenting the old count
    // as current.
    let mut label = count_button_text(&w.bounds);
    for _ in 0..12 {
        if label.as_deref() == Some(TAB_COUNT_WAITING_LABEL) {
            break;
        }
        settle_to_fixed_point(&mut w.app, &w.bounds, &w.runtime, Duration::from_secs(120));
        label = count_button_text(&w.bounds);
    }
    let warned: Vec<String> = test_init::captured_warnings()
        .into_iter()
        .map(|p| p.message)
        .filter(|m| m.contains("has not reported back"))
        .collect();
    eprintln!("[chrome-hung-write] label={label:?} warned={warned:?}");

    shutdown(w);

    assert_eq!(
        label.as_deref(),
        Some(TAB_COUNT_WAITING_LABEL),
        "with a write outstanding past the budget the button must show \
         {TAB_COUNT_WAITING_LABEL:?}; it shows {label:?}, which presents the pre-op count as \
         though it were current",
    );
    assert!(
        !warned.is_empty(),
        "the stalled write must be named in a WARN as well — nothing matching 'has not reported \
         back' was captured",
    );
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
#[path = "test_init/mod.rs"]
mod test_init;
