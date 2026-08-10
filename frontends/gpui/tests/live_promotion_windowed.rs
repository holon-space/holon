//! The windowed rung for live task-keyword promotion (task #68, closing the
//! half task #64 declared out of scope).
//!
//! #64 pinned the promotion at the ViewModel layer and on a cell-attached
//! editor, but nothing drove it through real rendered geometry: no test typed
//! `TODO ` into a row that had been CLICKED at its painted text, and none
//! pressed a real Cmd-Z afterwards. Those are exactly the two steps where a
//! promotion can look correct in the model and wrong on screen — the keyword
//! can leave the buffer without leaving the painted text, and the compound's
//! single undo entry can restore the store without the row re-rendering plain.
//!
//! So this test asserts the RENDERED pair on both sides of a real undo:
//!   * after typing, the row paints `milk` and its `state_toggle` shows `TODO`;
//!   * after Cmd-Z, the row paints `TODO milk` and the toggle is blank again.
//! `state_toggle_current` is `Some("")` for a plain row (the widget collapses
//! to a zero-width spacer), so the precondition, the promoted assertion and the
//! restored assertion are three distinct values — none of them can pass
//! vacuously.
//!
//! Rung: the windowed `SimUserDriver` (TestPlatform, real gpui dispatch, real
//! `text_center` click targeting). The paint and the undo keybinding both live
//! above the headless keystone, so no headless rung can reach them.

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui::AssetSource;
use gpui::TestApp;
use holon_api::EntityUri;
use holon_api::Key;
use holon_api::KeyChord;
use holon_frontend::focus_path::state_toggle_current;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::user_driver::UserDriver;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::window_slice::seed::PROMOTION_TARGET_CONTENT;
use holon_integration_tests::pbt::window_slice::seed::PROMOTION_TARGET_ID;
use holon_integration_tests::pbt::window_slice::seed::graft_promotion_target_row;
use holon_integration_tests::test_environment::TestEnvironment;

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;
use pbt_harness::sim_windowed_replay::SimUserDriver;

fn real_text_system() -> Arc<dyn gpui::PlatformTextSystem> {
    gpui_platform::current_platform(true).text_system()
}

/// Same cross-runtime fixed-point settle the other TestPlatform tests use.
fn settle(app: &mut TestApp, bounds: &BoundsRegistry, timeout: Duration) {
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

/// Every text this row currently PAINTS, by element id. Read from the bounds
/// registry rather than the view model, so a keyword that left the buffer but
/// not the screen is visible to the assertion.
fn painted_texts(bounds: &BoundsRegistry, entity: &str) -> Vec<(String, String)> {
    bounds.flush();
    let mut out: Vec<(String, String)> = bounds
        .all_elements()
        .iter()
        .filter(|(_, info)| info.entity_id.as_deref() == Some(entity))
        .filter(|(_, info)| matches!(info.widget_type.as_ref(), "rendered_text" | "editable_text"))
        .filter_map(|(id, info)| {
            info.displayed_text
                .as_ref()
                .map(|t| (id.to_string(), t.to_string()))
        })
        .collect();
    out.sort();
    out
}

/// The row's content as the resolved view model carries it — the PROJECTION of
/// the store, upstream of the live editor's own buffer. Comparing it against
/// what the row paints separates "the write never landed" from "the open editor
/// never re-seeded".
fn projected_text(
    root: &holon_frontend::view_model::ViewModel,
    entity_id: &EntityUri,
) -> Option<String> {
    use holon_frontend::view_model::ViewKind;
    if let ViewKind::EditableText { content, .. } | ViewKind::RenderedText { content, .. } =
        &root.kind
    {
        if root.entity_id().as_ref() == Some(entity_id) {
            return Some(content.clone());
        }
    }
    root.children()
        .iter()
        .find_map(|c| projected_text(c, entity_id))
}

#[test]
fn typed_keyword_paints_the_task_affordance_and_cmd_z_paints_it_plain_again() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = TestApp::with_text_system_and_assets(text_system, assets);

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    // Writes driven from the gpui thread reach Loro's `emit_change`, which
    // `tokio::spawn`s and therefore needs an entered reactor. The guard is held
    // for the whole test, so every future here is driven by a non-tokio
    // executor — `Runtime::block_on` panics inside an entered guard.
    let _reactor = runtime.enter();
    let env = futures::executor::block_on(async { TestEnvironment::new(runtime.clone()).unwrap() });
    futures::executor::block_on(async { env.start_app(true).await.expect("start_app") });
    futures::executor::block_on(graft_promotion_target_row(&env))
        .expect("graft the promotion target row");

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
                "Holon-TestPlatform-LivePromotion",
                cx,
            )
        })
        .expect("window opened");

    // Precondition, not a timing guess: the row must be PAINTED before a click
    // can seat a caret in it.
    let target_element = format!("block:{PROMOTION_TARGET_ID}");
    let boot_deadline = Instant::now() + Duration::from_secs(180);
    let mut painted = false;
    while Instant::now() < boot_deadline {
        settle(&mut app, &bounds, Duration::from_secs(30));
        painted = bounds
            .all_elements()
            .iter()
            .any(|(_, info)| info.entity_id.as_deref() == Some(target_element.as_str()));
        if painted {
            break;
        }
        eprintln!(
            "[promotion-boot] target not painted yet; {} elements",
            bounds.all_elements().len()
        );
    }
    assert!(
        painted,
        "boot precondition: the grafted promotion target row must paint before anything is typed"
    );

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

    // ALLOW(entity_uri_from_raw): the seed grafts this bare id; schemed here.
    let target = EntityUri::from_raw(PROMOTION_TARGET_ID);
    let root_id = holon_api::root_layout_block_uri();
    let root_tree = engine.snapshot_reactive(&root_id);
    let toggle = || state_toggle_current(&engine.snapshot_resolved(&root_id), &target, "main");
    // What the STORE holds, so a painted/stored disagreement names which layer
    // is wrong instead of blaming the render for a write that never landed.
    let projected = || projected_text(&engine.snapshot_resolved(&root_id), &target);

    // ── precondition: a plain row paints no task glyph ───────────────────
    assert_eq!(
        toggle().as_deref(),
        Some(""),
        "precondition: the seeded row is task-less, so its state_toggle renders blank"
    );

    // ── type `TODO ` at the head, through a REAL click ────────────────────
    // `click_entity` aims at `text_center` — the painted text box, not the
    // bullet — so this is the same targeting a user's click gets.
    futures::executor::block_on(driver.click_entity(&target, "main"))
        .expect("click the promotion target row's painted text");
    settle(&mut app, &bounds, Duration::from_secs(30));
    futures::executor::block_on(driver.send_raw_keystroke("home", &[]))
        .expect("caret to the head of the row");
    for key in ["T", "O", "D", "O", "space"] {
        futures::executor::block_on(driver.send_raw_keystroke(key, &[]))
            .unwrap_or_else(|e| panic!("typing {key:?} into the promotion target row: {e}"));
    }
    settle(&mut app, &bounds, Duration::from_secs(30));

    // ── the promoted rendering ───────────────────────────────────────────
    assert_eq!(
        toggle().as_deref(),
        Some("TODO"),
        "typing `TODO ` must paint the task affordance; the row's state_toggle still shows {:?}",
        toggle()
    );
    let promoted = painted_texts(&bounds, &target_element);
    assert!(
        promoted.iter().all(|(_, t)| t == PROMOTION_TARGET_CONTENT),
        "the keyword must leave the PAINTED text, not just the buffer; the row paints {promoted:?}"
    );
    assert!(
        !promoted.is_empty(),
        "the row painted no text at all — the assertion above would be vacuous"
    );

    // ── a REAL Cmd-Z ─────────────────────────────────────────────────────
    // The compound is ONE undo entry, so a single press must take back both
    // constituent writes: the keyword leaves `task_state` and returns to the
    // content it was stripped from.
    futures::executor::block_on(driver.send_key_chord(
        &root_id,
        &root_tree,
        &target,
        &KeyChord::new(&[Key::Cmd, Key::Char('z')]),
        Default::default(),
    ))
    .expect("cmd+z pressed into the promoted row");
    settle(&mut app, &bounds, Duration::from_secs(30));

    assert_eq!(
        toggle().as_deref(),
        Some(""),
        "cmd+z must take the task affordance back off the row; it still shows {:?}",
        toggle()
    );
    let restored = painted_texts(&bounds, &target_element);
    let expect_restored = format!("TODO {PROMOTION_TARGET_CONTENT}");
    assert!(
        restored.iter().all(|(_, t)| *t == expect_restored),
        "cmd+z must repaint the verbatim typed text {expect_restored:?}; the row paints \
         {restored:?} while the projection carries {:?}",
        projected()
    );
    assert!(
        !restored.is_empty(),
        "the row painted no text after undo — the assertion above would be vacuous"
    );

    drop(driver);
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
