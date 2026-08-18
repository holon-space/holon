//! Windowed probe for the dogfood report "navigating via arrow keys does not
//! work anymore" (2026-08-18).
//!
//! Two sibling rows under the Main focus root. Click the SECOND row, press a
//! bare `up`; focus must land on the FIRST row. Then press `down`; focus must
//! return to the second. Both directions cross a block boundary from a
//! single-line row, so `InputState` has nowhere to move the caret and must
//! `cx.propagate()` the action into `EditorView`'s bubble-phase
//! `handle_cross_block_nav`.
//!
//! Run: cargo nextest run -p holon-gpui --features holon-gpui/pbt --test
//! arrow_nav_windowed

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use holon_api::EntityUri;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::user_driver::UserDriver;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::window_slice::seed::graft_undo_blur_pair;
use holon_integration_tests::test_environment::TestEnvironment;

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;
use pbt_harness::sim_windowed_replay::SimUserDriver;

fn real_text_system() -> Arc<dyn gpui::PlatformTextSystem> {
    gpui_platform::current_platform(true).text_system()
}

fn settle(app: &mut HeadlessAppContext, bounds: &BoundsRegistry, timeout: Duration) {
    let start = Instant::now();
    let mut last_count = 0usize;
    let mut stable_iters = 0u32;
    while start.elapsed() < timeout {
        futures::executor::block_on(async { tokio::time::sleep(Duration::from_millis(20)).await });
        app.run_until_parked();
        app.advance_clock(Duration::from_secs(1));
        app.run_until_parked();
        bounds.flush();
        let count = bounds.all_elements().len();
        let still_loading = bounds
            .all_elements()
            .iter()
            .any(|(_, info)| info.widget_type.as_ref() == "loading");
        if count == last_count && count > 0 && !still_loading {
            stable_iters += 1;
            if stable_iters >= 5 {
                break;
            }
        } else {
            stable_iters = 0;
        }
        last_count = count;
    }
    futures::executor::block_on(async { tokio::task::yield_now().await });
    app.run_until_parked();
    bounds.flush();
}

static ONE_WINDOWED_APP_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The `entity_id`s of every `editable_text` the last painted frame reports as
/// holding window focus. Engine focus alone is not the user's experience: the
/// caret is where this says it is.
fn window_focused_editors(bounds: &BoundsRegistry) -> Vec<String> {
    let mut ids: Vec<String> = bounds
        .all_elements()
        .iter()
        .filter(|(_, info)| {
            info.widget_type.as_ref() == "editable_text" && info.focused == Some(true)
        })
        .map(|(_, info)| info.entity_id.as_deref().unwrap_or("<none>").to_string())
        .collect();
    ids.sort();
    ids
}

/// THE RUNG: a bare `up`/`down` at the row's text boundary crosses into the
/// sibling block, caret and all.
///
/// `home` first, so the caret sits at column 0 before the arrow. That is not
/// cosmetic — see the ignored arm below for the defect it steps around.
#[test]
fn arrow_keys_at_the_text_boundary_move_focus_between_sibling_blocks() {
    drive(true);
}

/// The same gesture WITHOUT the `home` — the caret is mid-line where a click
/// left it, which is how a user actually reaches for an arrow key.
///
/// Ignored because it fails in `gpui-component` (pin `8caad846`), not in Holon:
/// `InputState::up` yields the action to the parent only when the byte offset
/// did not change, and `move_vertical` at row 0 "moves" the caret by snapping
/// it to column 0 — so the first press is spent inside the row and Holon's
/// cross-block handler never runs. Fixing it means propagating on an unchanged
/// DISPLAY ROW instead, in that fork. See
/// docs/Testing/bugfunnel/entries/
/// 2026-08-18-arrow-keys-do-not-navigate-between-blocks.md
#[test]
#[ignore = "gpui-component InputState::up spends the first press snapping the caret to column 0"]
fn arrow_keys_from_mid_line_move_focus_between_sibling_blocks() {
    drive(false);
}

fn drive(home_first: bool) {
    let _serialized = ONE_WINDOWED_APP_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();

    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let _reactor = runtime.enter();
    let env = futures::executor::block_on(async { TestEnvironment::new(runtime.clone()).unwrap() });
    futures::executor::block_on(async { env.start_app(true).await.expect("start_app") });
    let (first_id, second_id) = futures::executor::block_on(graft_undo_blur_pair(&env, "arrownav"))
        .expect("graft the sibling row pair");

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
                "arrow-nav-windowed",
                cx,
            )
        })
        .expect("window opened");

    let second_element = format!("block:{second_id}");
    let boot_deadline = Instant::now() + Duration::from_secs(180);
    let mut painted = false;
    while Instant::now() < boot_deadline {
        settle(&mut app, &bounds, Duration::from_secs(30));
        painted = bounds
            .all_elements()
            .iter()
            .any(|(_, info)| info.entity_id.as_deref() == Some(second_element.as_str()));
        if painted {
            break;
        }
        eprintln!(
            "[arrow-nav-boot] second row not painted yet; {} elements",
            bounds.all_elements().len()
        );
    }
    assert!(painted, "boot precondition: both sibling rows must paint");

    let interaction_tx = debug_services
        .interaction_tx
        .get()
        .expect("interaction_tx set by the window interaction pump")
        .clone();
    // SAFETY: `app` outlives the driver and stays on this (gpui) thread.
    let driver = SimUserDriver::new(
        &app,
        rebind.window(),
        bounds.clone(),
        engine.clone(),
        runtime.handle().clone(),
        interaction_tx,
    );

    // ALLOW(entity_uri_from_raw): the seed grafts bare ids; schemed here.
    let first_uri = EntityUri::from_raw(&first_id);
    let second_uri = EntityUri::from_raw(&second_id);

    futures::executor::block_on(driver.click_entity(&second_uri, "main"))
        .expect("click the second row's painted text");
    settle(&mut app, &bounds, Duration::from_secs(30));
    assert_eq!(
        engine.focused_block(),
        Some(second_uri.clone()),
        "precondition: the click must focus the second row, else the arrow below starts nowhere"
    );
    assert_eq!(
        window_focused_editors(&bounds),
        vec![second_element.clone()],
        "precondition: the click must seat the caret in the second row's editor"
    );

    if home_first {
        futures::executor::block_on(driver.send_raw_keystroke("home", &[]))
            .expect("caret to the start of the row");
        settle(&mut app, &bounds, Duration::from_secs(30));
    }

    futures::executor::block_on(driver.send_raw_keystroke("up", &[]))
        .expect("the window must consume a bare up arrow");
    settle(&mut app, &bounds, Duration::from_secs(30));
    let first_element = format!("block:{first_id}");
    assert_eq!(
        engine.focused_block(),
        Some(first_uri.clone()),
        "a bare `up` on the first line of the second row must move focus to the previous sibling"
    );
    assert_eq!(
        window_focused_editors(&bounds),
        vec![first_element],
        "a bare `up` must move the CARET to the previous sibling, not only the engine's focus"
    );

    futures::executor::block_on(driver.send_raw_keystroke("down", &[]))
        .expect("the window must consume a bare down arrow");
    settle(&mut app, &bounds, Duration::from_secs(30));
    assert_eq!(
        engine.focused_block(),
        Some(second_uri),
        "a bare `down` on the last line of the first row must move focus back to the next sibling"
    );
    assert_eq!(
        window_focused_editors(&bounds),
        vec![second_element],
        "a bare `down` must move the CARET back to the next sibling"
    );

    drop(driver);
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(env);
}
