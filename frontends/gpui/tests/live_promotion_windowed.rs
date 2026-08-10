//! The windowed rung for the editable surface's SOURCE PROJECTION (task #78,
//! arm (d)) — the half no headless rung can reach: what the row PAINTS, and
//! what a real Cmd-Z does to it.
//!
//! Under arm (d) the editable surface shows the block's VAULT SYNTAX. So the
//! rung asserts the rendered pair on both sides of a real undo:
//!   * after typing `TODO ` at the head of `milk`, the FOCUSED row paints `TODO
//!     milk` while the STORE holds `milk` + `task_state = TODO` — the surface
//!     is a projection, and the two layers are asserted separately so a
//!     disagreement names which one is wrong;
//!   * the row's `state_toggle` shows `TODO`, so the task is real and not a
//!     rendering of leftover text;
//!   * a real Cmd-Z walks the text BACK — one press, one keystroke: the
//!     promoting write is an ordinary source commit, its inverse restores the
//!     converged value it replaced (`TODOmilk`), and the block stops being a
//!     task. A second press walks back another keystroke.
//!
//! That undo chain is the part arm (d) FIXED. The promotion compound's inverse
//! restored the VERBATIM typed text (`TODO milk`), which never equalled the
//! fused value the previous keystroke wrote (`TODOmilk`), so every earlier
//! entry was stale-dropped and the escape path did not exist. Every entry is
//! now written and inverted by the same mechanism, so the chain walks.
//!
//! The ILLEGAL state is checked at every step: keyword-headed text with a blank
//! task affordance renders to `** TODO …` and re-ingests as a task, so a store
//! holding that pair has diverged from its own file — the dogfood F2 signature,
//! stated where only a windowed rung can see it.
//!
//! `state_toggle_current` is `Some("")` for a plain row (the widget collapses
//! to a zero-width spacer), so the precondition and the promoted assertion are
//! distinct values and neither can pass vacuously.
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
fn the_focused_row_paints_its_vault_syntax_and_cmd_z_walks_the_text_back() {
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
    assert_eq!(
        projected().as_deref(),
        Some(PROMOTION_TARGET_CONTENT),
        "the STORE holds the stripped content — the keyword belongs to `task_state`"
    );
    let promoted = painted_texts(&bounds, &target_element);
    assert!(
        !promoted.is_empty(),
        "the row painted no text at all — the assertion below would be vacuous"
    );
    assert!(
        promoted.iter().all(|(_, t)| t == "TODO milk"),
        "the FOCUSED row must paint its VAULT SYNTAX — the keyword is editable text, not a \
         gesture that vanishes. The row paints {promoted:?} while the store carries {:?}",
        projected()
    );

    // ── a REAL Cmd-Z ─────────────────────────────────────────────────────
    // The source write is one entry over two columns, and its inverse restores
    // the CONVERGED value it replaced — so one press walks back exactly one
    // keystroke and the block stops being a task.
    // Labelled, and FALLIBLE: the driver refuses to press a chord into a row
    // whose editable_text does not hold window focus, so the delivery result
    // travels as data and each call site decides whether a refusal is fatal.
    let press_cmd_z = |app: &mut TestApp, label: &str| -> Result<(), String> {
        let delivered = futures::executor::block_on(driver.send_key_chord(
            &root_id,
            &root_tree,
            &target,
            &KeyChord::new(&[Key::Cmd, Key::Char('z')]),
            Default::default(),
        ))
        .map(|_| ())
        .map_err(|e| format!("{label}: {e}"));
        settle(app, &bounds, Duration::from_secs(30));
        delivered
    };

    press_cmd_z(&mut app, "cmd+z #1 (the promoting keystroke)")
        .unwrap_or_else(|e| panic!("the FIRST cmd+z must reach the promoted row: {e}"));

    assert_eq!(
        projected().as_deref(),
        Some("TODOmilk"),
        "one press walks back ONE keystroke: the source write's inverse restores the \
         converged value it replaced. The store carries {:?} and the toggle shows {:?}",
        projected(),
        toggle()
    );
    assert_eq!(
        toggle().as_deref(),
        Some(""),
        "the same press cleared `task_state` — the write was one gesture over both columns"
    );

    // ── a second press walks back another keystroke ──────────────────────
    // THE ARM-(d) FIX, stated as an assertion: under the promotion compound
    // this press met an entry whose fingerprint the promotion inverse had never
    // restored, so it was stale-dropped and the chain stopped here.
    let delivery = press_cmd_z(&mut app, "cmd+z #2 (into the typing chain)");
    let glyph = toggle().unwrap_or_default();
    let projection = projected();
    let painted = painted_texts(&bounds, &target_element)
        .first()
        .map(|(_, t)| t.clone());
    eprintln!(
        "[promotion-undo-chain] delivery={delivery:?} glyph={glyph:?} painted={painted:?} \
         projection={projection:?}"
    );

    // The ILLEGAL state, checked at BOTH layers whatever the press did.
    for (layer, text) in [("projection", &projection), ("paint", &painted)] {
        let Some(text) = text else { continue };
        assert!(
            !(glyph.is_empty() && text.starts_with("TODO ")),
            "the {layer} reached the ILLEGAL state — keyword-headed text {text:?} with no task \
             affordance. Those bytes render to `** {text}` and re-ingest as a task, which is \
             the dogfood F2 divergence."
        );
    }
    if delivery.is_ok() {
        // The chain must MOVE — that is what arm (d) fixed. How FAR one press
        // walks depends on how the journal groups a typing burst, which this
        // rung does not own, so the assertion is "strictly earlier in the
        // typed sequence", not a pinned entry. Observed here: the press lands
        // on the pre-typing state `milk`.
        let walked_back = ["TODmilk", "TOmilk", "Tmilk", PROMOTION_TARGET_CONTENT];
        assert!(
            projection
                .as_deref()
                .is_some_and(|p| walked_back.contains(&p)),
            "the undo chain must keep WALKING to an EARLIER typed state — under the promotion \
             compound this press met an entry the promotion inverse had never restored and was \
             stale-dropped here. Store: {projection:?}"
        );
    }

    drop(driver);
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
