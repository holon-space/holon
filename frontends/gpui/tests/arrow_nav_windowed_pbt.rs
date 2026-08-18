//! Windowed property rung for arrow-key navigation between blocks — the
//! generative companion to `arrow_nav_windowed.rs` (which pins the dogfood
//! repro as a fixed example).
//!
//! ## Why a WINDOWED rung, and why the headless keystone cannot see this
//!
//! The defect (`docs/Testing/bugfunnel/entries/2026-08-18-arrow-keys-do-not-
//! navigate-between-blocks.md`) lives entirely in a GPUI render builder:
//! `live_block::get_or_create_live_block` gave every main-panel block's
//! `ReactiveShell` a fresh `NavigationState::new()` whose `InputRouter` was
//! never handed the window's root tree, so `bubble_input` answered `None` to
//! every cross-block key. The headless composed keystone
//! (`general_e2e_composed_pbt`) runs its `ArrowNavigate` transition against a
//! SqlOnly SUT with NO frontend engine — `inv-focus-matches-ref` discloses
//! `EngineFocus::NoEngine` and SKIPS — and never instantiates `live_block` /
//! `ReactiveShell` / `InputRouter` at all. So no headless draw can observe the
//! router, and even the WINDOWED composed loop cannot: its wide fixture focuses
//! a block without seating an editor caret (arrow nav there is not
//! editor-boundary nav), and the reference model's `apply_arrow_navigate` does
//! not model the editor/caret following focus. This rung therefore drives the
//! REAL window over the keystone window-slice seed (`graft_undo_blur_pair`),
//! where the gesture is exactly what a user performs.
//!
//! ## The property
//!
//! Two single-line sibling rows under the Main focus root. From a generated
//! start row, a generated-length alternating walk of bare boundary arrows must
//! keep BOTH the engine's `focused_block` AND the painted window caret on the
//! outline neighbour the reference model resolves — `up` crosses to the
//! previous sibling, `down` to the next. `home` precedes every arrow so the
//! caret sits at column 0: the first vertical press from a mid-line caret is
//! spent inside `gpui-component`'s `InputState::up` snapping to the line edge
//! (Defect A, a separate fork bug — see the sibling file's `#[ignore]`d arm),
//! which this rung deliberately steps around so a red is unambiguously the
//! router defect.
//!
//! RED on the pre-fix tree: the first cross leaves focus on the start row
//! (`bubble_input -> None`), so the neighbour assertion fails. GREEN once the
//! shell shares the ambient `ctx.nav`.
//!
//! ⚠ `--test-threads=1` mandatory: gpui `TestApp` is not parallel-safe. A fresh
//! window is booted per case (≈15s), so the case budget is small (default 4,
//! override `PROPTEST_CASES`), mirroring `gpui_composed_windowed_loop.rs`.

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
use proptest::prelude::*;
use proptest::test_runner::Config;
use proptest::test_runner::TestCaseError;
use proptest::test_runner::TestRunner;

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;
use pbt_harness::sim_windowed_replay::SimUserDriver;

fn real_text_system() -> Arc<dyn gpui::PlatformTextSystem> {
    gpui_platform::current_platform(true).text_system()
}

static ONE_WINDOWED_APP_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

/// The `entity_id`s of every `editable_text` the last painted frame reports as
/// holding window focus — the caret's real position, not merely engine focus.
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

#[derive(Clone, Copy, Debug)]
enum Dir {
    Up,
    Down,
}

/// One generated case: a start row (0 = first/edit-target, 1 = second/blur
/// sibling) and how many boundary crossings to walk. The directions alternate
/// (a boundary arrow is only meaningful toward the neighbour that exists), so
/// the walk is a round-trip of generated length from a generated end.
fn case_strategy() -> impl Strategy<Value = (usize, u32)> {
    (0usize..=1, 1u32..=6)
}

#[test]
fn arrow_walk_keeps_focus_on_the_reference_outline_neighbour() {
    let _serialized = ONE_WINDOWED_APP_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();

    let cases: u32 = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);

    let mut runner = TestRunner::new(Config {
        cases,
        // A window re-boot per shrink candidate is ~15s; keep shrinking bounded.
        max_shrink_iters: 8,
        ..Config::default()
    });

    let result = runner.run(&case_strategy(), |(start, steps)| {
        drive_case(start, steps).map_err(|e| TestCaseError::fail(e))
    });

    if let Err(e) = result {
        panic!("[arrow-walk] windowed property failed (shrunk): {e}");
    }
}

/// Boot a fresh window, graft the sibling pair, and walk `steps` boundary
/// arrows from `start`, asserting engine + window-caret focus tracks the
/// reference outline neighbour after every press. Returns `Err` on an assertion
/// failure (so proptest can shrink) and `Ok(())` on success.
fn drive_case(start: usize, steps: u32) -> Result<(), String> {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let _reactor = runtime.enter();
    let env = futures::executor::block_on(async { TestEnvironment::new(runtime.clone()).unwrap() });
    futures::executor::block_on(async { env.start_app(true).await.expect("start_app") });
    let (first_id, second_id) = futures::executor::block_on(graft_undo_blur_pair(&env, "arrowpbt"))
        .expect("graft the sibling row pair");
    let rows = [first_id.clone(), second_id.clone()];

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
                "arrow-nav-windowed-pbt",
                cx,
            )
        })
        .expect("window opened");

    // Boot barrier: both rows must paint before any gesture.
    let boot_deadline = Instant::now() + Duration::from_secs(180);
    loop {
        settle(&mut app, &bounds, Duration::from_secs(30));
        let painted = |id: &str| {
            let el = format!("block:{id}");
            bounds
                .all_elements()
                .iter()
                .any(|(_, info)| info.entity_id.as_deref() == Some(el.as_str()))
        };
        if painted(&first_id) && painted(&second_id) {
            break;
        }
        if Instant::now() >= boot_deadline {
            return Err(format!(
                "boot: both rows must paint within 180s (elements={})",
                bounds.all_elements().len()
            ));
        }
    }

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

    let outcome = (|| -> Result<(), String> {
        // ALLOW(entity_uri_from_raw): the seed grafts bare ids; schemed here.
        let uri = |i: usize| EntityUri::from_raw(&rows[i]);
        let element = |i: usize| format!("block:{}", rows[i]);

        // Precondition: click the start row so the walk begins from a genuinely
        // focused editor (engine focus AND the painted caret).
        futures::executor::block_on(driver.click_entity(&uri(start), "main"))
            .map_err(|e| format!("click start row {start}: {e}"))?;
        settle(&mut app, &bounds, Duration::from_secs(30));
        if engine.focused_block() != Some(uri(start)) {
            return Err(format!(
                "precondition: click must focus start row {start} ({}), engine has {:?}",
                rows[start],
                engine.focused_block()
            ));
        }

        let mut pos = start;
        for step in 0..steps {
            // A boundary arrow toward the neighbour that exists: from the second
            // row press `up` (→ first), from the first press `down` (→ second).
            // This is exactly the crossing the reference model's
            // `apply_arrow_navigate` resolves for single-line siblings.
            let dir = if pos == 1 { Dir::Up } else { Dir::Down };
            let expected = if pos == 1 { 0 } else { 1 };

            futures::executor::block_on(driver.send_raw_keystroke("home", &[]))
                .map_err(|e| format!("step {step}: home: {e}"))?;
            settle(&mut app, &bounds, Duration::from_secs(30));

            let key = match dir {
                Dir::Up => "up",
                Dir::Down => "down",
            };
            futures::executor::block_on(driver.send_raw_keystroke(key, &[]))
                .map_err(|e| format!("step {step}: {key}: {e}"))?;
            settle(&mut app, &bounds, Duration::from_secs(30));

            if engine.focused_block() != Some(uri(expected)) {
                return Err(format!(
                    "step {step}: bare `{key}` from row {pos} ({}) must move engine focus to the \
                     outline neighbour row {expected} ({}); engine has {:?}",
                    rows[pos],
                    rows[expected],
                    engine.focused_block()
                ));
            }
            let caret = window_focused_editors(&bounds);
            if caret != vec![element(expected)] {
                return Err(format!(
                    "step {step}: bare `{key}` must move the CARET to row {expected} ({}); \
                     painted focus = {caret:?}",
                    rows[expected]
                ));
            }
            pos = expected;
        }
        Ok(())
    })();

    drop(driver);
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(env);

    outcome
}
