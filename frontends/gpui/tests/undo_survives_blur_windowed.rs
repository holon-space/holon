//! The windowed rung for dogfood finding F1: an undone content edit must
//! survive the blur that follows it.
//!
//! THE SHAPE (reproduced twice by the dogfood-explorer against a real app):
//! click a row, append two characters, press Cmd-Z — the store AND the render
//! both show the pre-edit text, so nothing looks wrong — then click a DIFFERENT
//! row. The blur commits the open editor's buffer, which still holds the
//! pre-undo text, straight over the restored store. The undo is silently gone
//! and no error is logged anywhere, because every layer agreed right up to the
//! blur.
//!
//! TWIN PAIR, and the pair is the point. A FOCUSED editor is skipped by the
//! render backstop (`converge_on_render`) so in-flight typing is never yanked,
//! which leaves it exactly one convergence channel — and WHICH channel that is
//! depends on the storage mode:
//!   * Loro/cell-attached: the entity `Cell`'s remote-delta subscription, which
//!     survives row-set rebuilds.
//!   * SqlOnly (the SHIPPED DEFAULT, `crdt.enabled` unset): the per-row data
//!     subscription, which any row-set rebuild orphans, plus the undo/redo
//!     `ReseedGesture`.
//! The dogfood ran the shipped default. `TestEnvironment` defaults to Loro, so
//! a single-arm rung here would have exercised the immune mode and reported a
//! green that means nothing about the finding — hence both arms, with the mode
//! asserted in each.
//!
//! Why this rung and not the headless keystone: the whole defect lives in the
//! seam between the store and a FOCUSED editor's visible buffer, and the
//! headless mirror cannot express either half of it —
//! `HeadlessEditorMirror::converge_editor` is called unconditionally from the
//! harness settle, so it models the data-sync loop as always delivering (a
//! focused editor there can never hold a stale buffer), and the mirror drives
//! no blur commit at all (`vm_commit_edit` commits per keystroke; `on_blur` is
//! never called headlessly). Both absences are the remedy named in the
//! BugFunnel row.
//!
//! Otherwise this is deliberately the GENERIC content-edit twin of
//! `live_promotion_windowed.rs`: same boot, same real click targeting, same
//! real Cmd-Z. What differs is what gets typed (a plain suffix — no keyword, no
//! promotion compound) and the blur click at the end.
//!
//! Run: cargo test -p holon-gpui --features pbt --test
//! undo_survives_blur_windowed

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui::AssetSource;
use gpui::TestApp;
use holon_api::EntityUri;
use holon_api::Key;
use holon_api::KeyChord;
use holon_api::Value;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::user_driver::UserDriver;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::window_slice::seed::UNDO_EDIT_CONTENT;
use holon_integration_tests::pbt::window_slice::seed::graft_undo_blur_pair;
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

/// What the STORE holds for `id`, read straight from `block_raw`. This is the
/// oracle the finding is about: the render and the projection both agreed with
/// the undo, and it is the stored row that silently went backwards.
fn stored_content(env: &TestEnvironment, id: &str) -> String {
    let uri = EntityUri::block(id).to_string();
    let sql = format!("SELECT content FROM block_raw WHERE id = '{uri}'");
    let rows = futures::executor::block_on(async {
        env.engine()
            .execute_query(sql, HashMap::new(), None)
            .await
            .expect("read block_raw.content")
    });
    match rows.first().and_then(|r| r.get("content")) {
        Some(Value::String(s)) => s.clone(),
        other => panic!("block_raw has no string content for {uri}: {other:?}"),
    }
}

/// Every text the row currently PAINTS. Used to prove the undo was visible —
/// the finding's sting is that nothing looked wrong before the blur.
fn painted_texts(bounds: &BoundsRegistry, entity: &str) -> Vec<String> {
    bounds.flush();
    let mut out: Vec<String> = bounds
        .all_elements()
        .iter()
        .filter(|(_, info)| info.entity_id.as_deref() == Some(entity))
        .filter(|(_, info)| matches!(info.widget_type.as_ref(), "rendered_text" | "editable_text"))
        .filter_map(|(_, info)| info.displayed_text.as_ref().map(|t| t.to_string()))
        .collect();
    out.sort();
    out
}

/// One arm of the rung. `loro` picks the storage mode; `suffix` keys the row
/// ids so the two arms never share a `local_edit_epoch` entry.
fn drive_undo_then_blur(
    loro: bool,
    suffix: &str,
    window_title: &'static str,
    rebuild_rowset_first: bool,
) {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = TestApp::with_text_system_and_assets(text_system, assets);

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    // Writes driven from the gpui thread reach Loro's `emit_change`, which
    // `tokio::spawn`s and therefore needs an entered reactor.
    let _reactor = runtime.enter();
    let env = futures::executor::block_on(async { TestEnvironment::new(runtime.clone()).unwrap() });
    // Mode BEFORE boot: the wiring is read once at `start_app`.
    env.set_enable_loro(loro);
    futures::executor::block_on(async { env.start_app(true).await.expect("start_app") });
    assert_eq!(
        env.loro_enabled(),
        loro,
        "this arm must boot the storage mode it claims — a rung that silently ran the other \
         mode's convergence channel proves nothing about the finding"
    );
    let (edit_id, blur_id) = futures::executor::block_on(graft_undo_blur_pair(&env, suffix))
        .expect("graft the undo/blur row pair");

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
                window_title,
                cx,
            )
        })
        .expect("window opened");

    // Precondition, not a timing guess: both rows must be PAINTED before a
    // click can seat a caret in one and move focus to the other.
    let edit_element = format!("block:{edit_id}");
    let blur_element = format!("block:{blur_id}");
    let boot_deadline = Instant::now() + Duration::from_secs(180);
    let mut painted = false;
    while Instant::now() < boot_deadline {
        settle(&mut app, &bounds, Duration::from_secs(30));
        let ids: Vec<String> = bounds
            .all_elements()
            .iter()
            .filter_map(|(_, info)| info.entity_id.as_ref().map(|e| e.to_string()))
            .collect();
        painted = ids.iter().any(|e| e == &edit_element) && ids.iter().any(|e| e == &blur_element);
        if painted {
            break;
        }
        eprintln!(
            "[undo-blur-boot] rows not both painted yet; {} elements",
            bounds.all_elements().len()
        );
    }
    assert!(
        painted,
        "boot precondition: both grafted rows must paint before anything is typed"
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

    // ALLOW(entity_uri_from_raw): the seed grafts these bare ids; schemed here.
    let edit_target = EntityUri::from_raw(&edit_id);
    let blur_target = EntityUri::from_raw(&blur_id);
    let root_id = holon_api::root_layout_block_uri();
    let root_tree = engine.snapshot_reactive(&root_id);

    assert_eq!(
        stored_content(&env, &edit_id),
        UNDO_EDIT_CONTENT,
        "precondition: the grafted row starts at its seeded content"
    );

    // Optional provocation: focus the OTHER row first, so the edit row's
    // editor is built (and its per-row data subscription bound) only after the
    // row set has already been rebuilt once. The orphaning that leaves a
    // focused SqlOnly editor without a data-sync channel needs such a rebuild.
    if rebuild_rowset_first {
        futures::executor::block_on(driver.click_entity(&blur_target, "main"))
            .expect("pre-click the sibling row to rebuild the row set");
        settle(&mut app, &bounds, Duration::from_secs(30));
    }

    // ── append `QQ` through a REAL click + REAL keystrokes ────────────────
    futures::executor::block_on(driver.click_entity(&edit_target, "main"))
        .expect("click the edit target row's painted text");
    settle(&mut app, &bounds, Duration::from_secs(30));
    futures::executor::block_on(driver.send_raw_keystroke("end", &[]))
        .expect("caret to the end of the row");
    for key in ["Q", "Q"] {
        futures::executor::block_on(driver.send_raw_keystroke(key, &[]))
            .unwrap_or_else(|e| panic!("typing {key:?} into the edit target row: {e}"));
    }
    settle(&mut app, &bounds, Duration::from_secs(30));

    let typed = format!("{UNDO_EDIT_CONTENT}QQ");
    assert_eq!(
        stored_content(&env, &edit_id),
        typed,
        "the typed suffix must reach the store — otherwise the undo below has nothing to take \
         back and every assertion after it is vacuous"
    );

    // ── a REAL Cmd-Z ─────────────────────────────────────────────────────
    futures::executor::block_on(driver.send_key_chord(
        &root_id,
        &root_tree,
        &edit_target,
        &KeyChord::new(&[Key::Cmd, Key::Char('z')]),
        Default::default(),
    ))
    .expect("cmd+z pressed into the edited row");
    settle(&mut app, &bounds, Duration::from_secs(30));

    // The state the dogfooder saw and trusted: store and render both undone.
    assert_eq!(
        stored_content(&env, &edit_id),
        UNDO_EDIT_CONTENT,
        "cmd+z must restore the pre-edit content in the store"
    );
    let after_undo = painted_texts(&bounds, &edit_element);
    assert!(
        !after_undo.is_empty(),
        "the row painted no text after undo — the next assertion would be vacuous"
    );
    assert!(
        after_undo.iter().all(|t| t == UNDO_EDIT_CONTENT),
        "cmd+z must repaint the pre-edit text; the row paints {after_undo:?}"
    );

    // ── the blur: click a DIFFERENT row ──────────────────────────────────
    // Focus leaves the edited row, so its open editor commits. If that commit
    // carries the pre-undo buffer, the undo is silently thrown away.
    futures::executor::block_on(driver.click_entity(&blur_target, "main"))
        .expect("click the sibling row to blur the edited one");
    settle(&mut app, &bounds, Duration::from_secs(30));

    // NON-VACUITY: the whole finding is about what a blur commits, so a click
    // that never moved focus would make the assertion below pass for the wrong
    // reason. Focus is the engine-level fact the editor's blur edge follows.
    assert_eq!(
        engine.focused_block().as_ref(),
        Some(&blur_target),
        "the blur click must actually move focus off the edited row — otherwise no blur commit \
         ran and the assertion below proves nothing"
    );

    assert_eq!(
        stored_content(&env, &edit_id),
        UNDO_EDIT_CONTENT,
        "F1: clicking away from an UNDONE row must not resurrect the undone text — the open \
         editor's stale buffer committed over the restored store (the row paints {:?})",
        painted_texts(&bounds, &edit_element)
    );

    drop(driver);
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
}

/// THE ARM THE FINDING IS ABOUT: the shipped default. A focused SqlOnly editor
/// has no cell to converge from — only the row-set-orphanable data
/// subscription and the undo/redo re-seed.
#[test]
fn undone_content_edit_survives_clicking_another_row_sqlonly() {
    drive_undo_then_blur(
        false,
        "sqlonly",
        "Holon-TestPlatform-UndoBlur-SqlOnly",
        false,
    );
}

/// The same SqlOnly arm with the editor mounted AFTER a row-set rebuild — the
/// state in which the per-row data subscription is orphaned, so the undo
/// re-seed is the row's only remaining convergence channel.
#[test]
fn undone_content_edit_survives_clicking_another_row_sqlonly_after_rebuild() {
    drive_undo_then_blur(
        false,
        "sqlonly-rebuilt",
        "Holon-TestPlatform-UndoBlur-SqlOnly-Rebuilt",
        true,
    );
}

/// The control. Cell-attached editors converge through the entity `Cell`, which
/// no row-set rebuild orphans — the dogfood observed this arm behaving.
#[test]
fn undone_content_edit_survives_clicking_another_row_loro() {
    drive_undo_then_blur(true, "loro", "Holon-TestPlatform-UndoBlur-Loro", false);
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
