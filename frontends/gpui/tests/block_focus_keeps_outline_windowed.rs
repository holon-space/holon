//! Dogfood escape (Martin, on-device DN2103, 2026-08-28): tapping a block row
//! on a phone-sized window makes the whole outline disappear — the page title
//! and every block row stop painting, only the "Linked references" accordion
//! survives, and the block the user just tapped no longer exists as a render
//! target, so nothing they type has anywhere to go.
//!
//! The tap is the trigger; the cause is the SHORT BOX it leaves behind. Two
//! things shrink the main panel at once: the soft keyboard's safe-area inset
//! (~290 logical px), and the open-tabs strip plus page-ancestor breadcrumb
//! that `HolonApp::render` mounts the first time `UiState::focused_block`
//! changes (`frontends/gpui/src/lib.rs`, the two focus-change blocks around
//! L875-930 — both latches start `None` next to a `None` focus, so a cold start
//! never resolves that chrome until the first tap). A short panel paints no
//! rows at all, which is what the user sees.
//!
//! Two tests, one defect:
//!
//! - `raising_the_keyboard_must_not_hide_the_block_rows` — Martin's acceptance
//!   criterion, through the production inset path. The keyboard-down frame is
//!   the control, so a failure belongs to raising it.
//! - `a_short_window_still_paints_the_outline` — the same end state reached
//!   with no tap and no keyboard, purely by opening a short window. It isolates
//!   the geometry from the focus path.
//!
//! Why this also kills typing: `Window::handle_input` only arms an input
//! handler while the focused element is being PAINTED (`gpui/src/window.rs`,
//! `debug_assert_paint` + `is_focused`), and the end of each draw re-arms the
//! platform window from what that frame pushed. A focused row that stops being
//! painted takes the input handler with it — the fork's "no input handler is
//! set, so no editor is focused" at frame rate — and a re-tap cannot recover it
//! because the row still is not painted. Keeping the rows painted is the same
//! fix as keeping the handler.
//!
//! Why a WINDOW and not the keystone: the chrome bars, the safe-area padding,
//! the panel's flex allocation and the virtualized row measurement are all
//! painted geometry. The headless keystone has no window, so it structurally
//! cannot see any of it. The windowed rung is the lowest one that can.
//!
//! Run: `cargo nextest run -p holon-gpui --test
//! block_focus_keeps_outline_windowed --features holon-gpui/pbt --no-fail-fast`
//! (nextest gives each test its own process, which the gpui test platform's
//! thread-local state requires).

use std::collections::BTreeSet;
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
use holon_integration_tests::pbt::window_slice::builders::window_wide;
use holon_integration_tests::pbt::window_slice::seed::graft_displayed_text_tree;
use holon_integration_tests::test_environment::TestEnvironment;
use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::composition::CapMap;

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;
use pbt_harness::sim_windowed_replay::SimUserDriver;

/// The grafted outline: a page with a `parent` row owning `c1` and `c2`.
/// `graft_displayed_text_tree` fixes these ids.
const OUTLINE_ROWS: [&str; 3] = ["block:parent", "block:c1", "block:c2"];
/// The row the test taps. A child, not the page title, so the tap is the same
/// gesture Martin used: putting the caret in an ordinary outline row.
const TAPPED_ROW: &str = "block:c1";

/// A SHORT phone-shaped window — the reproducing geometry. Measured behaviour
/// of the shipped main panel against this three-row fixture, as the window
/// shrinks (`lane-logs/sweep-boxes.log`):
///
/// | window  | panel box | outline region | rows painted |
/// |---------|-----------|----------------|--------------|
/// | 393x852 | 790       | 528.5          | 3            |
/// | 393x600 | 538       | 359.5          | 1 (the last) |
/// | 393x500 | 438       | 292.5          | **0**        |
///
/// The rows do not scroll out of a full region; they stop being drawn while
/// 292px of empty panel sits there. A tall window hides it, which is why the
/// size is pinned here.
const PHONE_WINDOW: &str = "393x500";

/// A comfortable phone window with the keyboard DOWN — the control geometry for
/// `raising_the_keyboard_must_not_hide_the_block_rows`.
const TALL_WINDOW: &str = "393x852";
/// How much vertical room an open soft keyboard costs, in logical px. The
/// device measured a 792-physical bottom inset at ~2.75x ≈ 288 logical, plus
/// the ~50px of tab-strip and breadcrumb chrome the same focus change mounts.
/// 340 of 852 is the ~40% cut the device takes.
const KEYBOARD_INSET_PX: f32 = 340.0;

/// The main panel's live_block id in the shipped layout.
const MAIN_PANEL: &str = "block:default-main-panel";
/// Room the panel must have before its empty frame can be blamed on the
/// frontend rather than on a fixture that left it no space. Three 36px rows
/// need ~110px; 200 is a wide margin above that.
const MIN_PANEL_BOX_PX: f32 = 200.0;

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

/// The outline rows the window is currently PAINTING: a grafted row counts only
/// when it registered a box with a real height. Registration alone is not
/// enough — the panel registers bounds for rows it lays out below the fold —
/// so height is what separates "on the screen" from "in the tree".
fn painted_outline_rows(sut: &CapMap, runtime: &tokio::runtime::Runtime) -> BTreeSet<String> {
    let elements = runtime.block_on(async { sut.rendered_elements().await });
    elements
        .iter()
        .filter(|e| e.height > 0.0 && e.width > 0.0)
        .filter_map(|e| e.entity_id.as_ref())
        .map(|u| u.as_str().to_string())
        .filter(|id| OUTLINE_ROWS.contains(&id.as_str()))
        .collect()
}

/// Height of the main panel's own box — the room the outline had to draw in.
fn panel_box_height(sut: &CapMap, runtime: &tokio::runtime::Runtime) -> f32 {
    let elements = runtime.block_on(async { sut.rendered_elements().await });
    elements
        .iter()
        .filter(|e| e.entity_id.as_ref().map(EntityUri::as_str) == Some(MAIN_PANEL))
        .map(|e| e.height)
        .fold(0.0f32, f32::max)
}

/// Print every element box the frame registered, tallest first. The interesting
/// ones are the main panel and whatever sits inside it: when the outline stops
/// painting, this is what says whether the panel got a box at all, and how much
/// of it the outline region was given.
fn dump_boxes(label: &str, sut: &CapMap, runtime: &tokio::runtime::Runtime) {
    let mut elements = runtime.block_on(async { sut.rendered_elements().await });
    elements.sort_by(|a, b| b.height.total_cmp(&a.height));
    for e in elements.iter().take(30) {
        eprintln!(
            "[boxes/{label}] {:<22} {:<38} y={:7.1} h={:7.1} el={}",
            e.widget_type,
            e.entity_id
                .as_ref()
                .map(EntityUri::as_str)
                .unwrap_or("<no entity>"),
            e.y,
            e.height,
            e.el_id,
        );
    }
}

/// MARTIN'S ACCEPTANCE CRITERION (2026-08-28): "the blocks disappear from view
/// when the keyboard pops up" — they must not.
///
/// The keyboard is raised the way the platform raises it. On Android the window
/// does NOT resize when the IME appears; the bottom safe-area inset grows and
/// `HolonApp::render` applies it as `.pb()` on the page container
/// (`frontends/gpui/src/lib.rs`), shrinking the box every panel lays out in
/// while the window keeps its size. `RebindHandle::set_safe_area_bottom` sets
/// that same field, so this drives the production code path rather than
/// approximating it with a window resize (which would also move the viewport,
/// something the real IME never does).
///
/// Why this is also the input-handler gate: `Window::handle_input` only arms an
/// input handler while the focused element is being PAINTED
/// (`gpui/src/window.rs:4059`, `debug_assert_paint` + `is_focused`), and the
/// end of each draw re-arms the platform window from what that frame pushed
/// (`window.rs:2406`). A focused row that stops being painted therefore takes
/// the input handler with it — which is the fork's "no input handler is set, so
/// no editor is focused" at frame rate, and why a re-tap cannot recover it.
/// Keeping the rows painted is the same fix as keeping the handler.
#[test]
fn raising_the_keyboard_must_not_hide_the_block_rows() {
    // SAFETY: single-threaded test setup, before any window or runtime thread
    // reads the environment.
    unsafe { std::env::set_var("HOLON_INITIAL_WINDOW_SIZE", TALL_WINDOW) };

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
                "Holon-Keyboard-Inset",
                cx,
            )
        })
        .expect("window opened");
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    runtime
        .block_on(graft_displayed_text_tree(&env))
        .expect("graft the outline the panel draws");
    runtime
        .block_on(env.wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)));
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(120));

    let sut = window_wide(Box::new(bounds.clone()), engine.clone());

    let app_ptr: *const HeadlessAppContext = &app;
    let driver = SimUserDriver::new(
        app_ptr,
        rebind.window(),
        bounds.clone(),
        engine.clone(),
        runtime.handle().clone(),
        debug_services
            .interaction_tx
            .get()
            .expect("interaction_tx set by the window interaction pump")
            .clone(),
    );

    // Tap a row to put the caret in it — the gesture that raises the keyboard.
    let tapped = EntityUri::block(TAPPED_ROW.trim_start_matches("block:"));
    runtime
        .block_on(async { driver.click_entity(&tapped, "main").await })
        .expect("tap the outline row to put the caret in it");
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(120));

    // CONTROL. With the keyboard down the frame is fine, so a failure after the
    // inset belongs to the inset and not to the tap or the fixture.
    let before = painted_outline_rows(&sut, &runtime);
    let panel_before = panel_box_height(&sut, &runtime);
    eprintln!("[keyboard-inset] keyboard DOWN: panel_h={panel_before:.1} painted={before:?}");
    assert_eq!(
        before.len(),
        OUTLINE_ROWS.len(),
        "with the keyboard DOWN the panel ({panel_before:.1}px) must paint all {} rows, else the \
         claim below cannot be attributed to raising it; painted: {before:?}",
        OUTLINE_ROWS.len(),
    );

    // Raise the keyboard: grow the bottom safe-area inset, which is exactly
    // what the platform does to the app on Android.
    app.update(|cx| rebind.set_safe_area_bottom(KEYBOARD_INSET_PX, cx));
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(120));

    let after = painted_outline_rows(&sut, &runtime);
    let panel_after = panel_box_height(&sut, &runtime);
    let lost: Vec<&String> = before.difference(&after).collect();
    dump_boxes("keyboard-up", &sut, &runtime);
    eprintln!(
        "[keyboard-inset] keyboard UP ({KEYBOARD_INSET_PX}px): panel_h={panel_after:.1} \
         painted={after:?} lost={lost:?}"
    );

    assert!(
        lost.is_empty(),
        "raising the keyboard removed {} block row(s) {lost:?} from the screen. The panel still \
         has {panel_after:.1}px to draw in, so there was room for them. This is what Martin sees \
         on the device — and because gpui only arms an input handler while the focused element is \
         painted, the focused row leaving the frame is also what kills typing.",
        lost.len(),
    );
    assert!(
        after.contains(TAPPED_ROW),
        "the focused row ({TAPPED_ROW}) must still be painted with the keyboard up — it is the \
         element gpui re-arms the input handler from every frame, so once it stops being painted \
         the held IME edits can never drain; painted: {after:?}",
    );

    drop(driver);
    drop(sut);
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(env);
}

#[test]
fn a_short_window_still_paints_the_outline() {
    // Read by `launch_holon_window_impl`; must be set before the window opens.
    // SAFETY: single-threaded test setup, before any window or runtime thread
    // reads the environment.
    // An already-set value wins, so the height can be swept from the shell
    // without a rebuild while the reproducing geometry is being hunted.
    let window_size =
        std::env::var("HOLON_INITIAL_WINDOW_SIZE").unwrap_or_else(|_| PHONE_WINDOW.to_string());
    unsafe { std::env::set_var("HOLON_INITIAL_WINDOW_SIZE", &window_size) };

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
                "Holon-Block-Focus-Outline",
                cx,
            )
        })
        .expect("window opened");
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    // Graft a plain page with a three-row outline and land Main on it, so the
    // panel draws an outline (the booted vault lands on the journals feed,
    // which shows only Page-tagged children).
    runtime
        .block_on(graft_displayed_text_tree(&env))
        .expect("graft the outline the panel draws");
    runtime
        .block_on(env.wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60)));
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(120));

    let sut = window_wide(Box::new(bounds.clone()), engine.clone());

    // THE CLAIM. The panel has a real box and three one-line rows to put in it,
    // so every row must be on screen.
    //
    // The panel box is read first and asserted separately, because it is what
    // separates the two ways this can fail. A panel with NO box would mean the
    // fixture never gave the outline any room — the test's own fault. A panel
    // with a 290px box that paints none of three 36px rows is the defect.
    let panel_height = panel_box_height(&sut, &runtime);
    let before = painted_outline_rows(&sut, &runtime);
    dump_boxes("before", &sut, &runtime);
    eprintln!(
        "[block-focus-outline] window={window_size} panel_h={panel_height:.1} painted={before:?}"
    );
    // Diagnostic probe, off by default. Scrolls the emptied panel back up and
    // re-reads it: rows that reappear mean the list's visible window had run
    // past the end of its content; rows that stay away mean they were never
    // laid out at full height. Set `HOLON_PROBE_SCROLL_UP=1` to run it.
    if std::env::var("HOLON_PROBE_SCROLL_UP").is_ok() {
        let probe = SimUserDriver::new(
            &app as *const HeadlessAppContext,
            rebind.window(),
            bounds.clone(),
            engine.clone(),
            runtime.handle().clone(),
            debug_services
                .interaction_tx
                .get()
                .expect("interaction_tx")
                .clone(),
        );
        for _ in 0..10 {
            let _ = runtime.block_on(async { probe.scroll_at(200.0, 150.0, 0.0, 20.0).await });
        }
        settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(60));
        let scrolled = painted_outline_rows(&sut, &runtime);
        eprintln!("[probe/scroll-up] painted after scrolling to the top: {scrolled:?}");
        dump_boxes("scrolled", &sut, &runtime);
    }

    assert!(
        panel_height > MIN_PANEL_BOX_PX,
        "the fixture must give the main panel a real box, else nothing below judges the frontend; \
         got {panel_height:.1}px at window {window_size}",
    );
    assert_eq!(
        before.len(),
        OUTLINE_ROWS.len(),
        "the main panel has a {panel_height:.1}px box and three one-line rows to draw, and painted \
         {} of them: {before:?}. A short window must show fewer rows, never none — the outline the \
         user came for is simply not on the screen.",
        before.len(),
    );

    let app_ptr: *const HeadlessAppContext = &app;
    let interaction_tx = debug_services
        .interaction_tx
        .get()
        .expect("interaction_tx set by the window interaction pump")
        .clone();
    let driver = SimUserDriver::new(
        app_ptr,
        rebind.window(),
        bounds.clone(),
        engine.clone(),
        runtime.handle().clone(),
        interaction_tx,
    );

    // The gesture under test: a plain tap on an outline row, through the
    // production click path — the same thing a finger does on the device.
    let tapped = EntityUri::block(TAPPED_ROW.trim_start_matches("block:"));
    runtime
        .block_on(async { driver.click_entity(&tapped, "main").await })
        .expect("tap the outline row to put the caret in it");
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(120));

    let after = painted_outline_rows(&sut, &runtime);
    let lost: Vec<&String> = before.difference(&after).collect();

    eprintln!(
        "[block-focus-outline/after-tap] window={window_size} before={before:?} after={after:?} \
         lost={lost:?}"
    );

    assert!(
        after.contains(TAPPED_ROW),
        "the row that just took focus ({TAPPED_ROW}) must still be painted — a caret in a row \
         with no render target has nowhere to put what the user types; painted after the tap: \
         {after:?}",
    );
    assert!(
        lost.is_empty(),
        "focusing a block row must not un-paint blocks: {} row(s) {lost:?} were on screen before \
         the tap and are gone after it (before={before:?}, after={after:?})",
        lost.len(),
    );

    drop(driver);
    drop(sut);
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
