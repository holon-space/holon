//! The windowed rung for dogfood finding F1 of the re-entry pass (task #99):
//! a block promoted DURING its own editing session must not fold its keyword
//! into its title when focus leaves it.
//!
//! THE SHAPE (15/15 reproductions against a real app): click a plain row, type
//! `TODO ` at the head — the store promotes correctly (`content` stripped,
//! `task_state = TODO`) — then click a DIFFERENT row. The blur commit re-writes
//! the editor's SURFACE buffer (`TODO alpha two`, which under arm (d) is vault
//! syntax, not content) through the CONTENT channel. The store then holds
//! `content = "TODO alpha two"` AND `task_state = TODO`, which renders to
//! `* TODO TODO alpha two` on disk and re-ingests with the keyword in the
//! title.
//!
//! WHY IT ESCAPED EVERY EXISTING RUNG: arm (d) routes the per-keystroke commit
//! through `EditorViewModel::commits_as_source`, and every arm-(d) test drives
//! that funnel. The blur/pending-commit funnel
//! (`EditorViewModel::on_blur` / `pending_commit_intent` →
//! `ViewEventHandler::handle_text_sync`) builds its own `set_field` from the
//! handler's own `field`, which is `content` for an editable-text node — it has
//! never consulted `Surface` at all. Only a rung that delivers a REAL focus
//! transfer after a promotion crosses that funnel; a rung that stops at the
//! last keystroke passes on the keystroke's own (correctly routed) source
//! commit. A cheap `mount_editor` stub cannot substitute: those editors are
//! built with no operations, so `handle_text_sync` can never produce an
//! `Execute` and the rung passes vacuously (proven, see lane-report-99.md).
//!
//! MODE: SqlOnly, the SHIPPED DEFAULT (`crdt.enabled` unset) — the mode the
//! dogfood ran. `TestEnvironment` defaults to Loro, where the blur `set_field`
//! is dropped as a redundant second content writer
//! (`ViewEventHandler::loro_content_writer`), so a Loro-only rung would report
//! a green that means nothing about the finding. The Loro arm is kept as a
//! labelled control.
//!
//! Run: cargo test -p holon-gpui --features pbt --test
//! task_keyword_blur_windowed

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use gpui::AssetSource;
use gpui::TestApp;
use holon_api::EntityUri;
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

fn sql_rows(env: &TestEnvironment, sql: String) -> Vec<HashMap<Arc<str>, Value>> {
    futures::executor::block_on(async {
        env.engine()
            .execute_query(sql, HashMap::new(), None)
            .await
            .expect("query the store")
    })
}

/// The two columns the finding is about, read straight from `block_raw`.
fn stored_pair(env: &TestEnvironment, id: &str) -> (String, Option<String>) {
    let uri = EntityUri::block(id).to_string();
    let rows = sql_rows(
        env,
        format!(
            "SELECT content, json_extract(properties, '$.task_state') AS task_state \
             FROM block_raw WHERE id = '{uri}'"
        ),
    );
    let row = rows.first().expect("the edited block must exist");
    let content = match row.get("content") {
        Some(Value::String(s)) => s.clone(),
        other => panic!("block_raw has no string content for {uri}: {other:?}"),
    };
    let task_state = match row.get("task_state") {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    };
    (content, task_state)
}

/// How many history rows this block has accumulated. The vacuity guard: the
/// blur must actually DISPATCH something, otherwise "the store is still clean"
/// proves only that no commit ran.
fn history_rows(env: &TestEnvironment, id: &str) -> i64 {
    let uri = EntityUri::block(id).to_string();
    let rows = sql_rows(
        env,
        format!("SELECT COUNT(*) AS n FROM block_history WHERE block_id = '{uri}'"),
    );
    match rows.first().and_then(|r| r.get("n")) {
        Some(Value::Integer(n)) => *n,
        other => panic!("block_history count is not an integer: {other:?}"),
    }
}

/// Every text the row currently PAINTS.
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

/// Every org line under the vault root that names this row's text, so the
/// disk-side signature (`* TODO TODO alpha two`) is asserted where it lands
/// rather than inferred. Supplementary to the store assertion: an empty list
/// means write-back has not run yet, never that the file is correct.
fn org_headlines(env: &TestEnvironment, needle: &str) -> Vec<String> {
    fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "org") {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    out.extend(text.lines().map(str::to_string));
                }
            }
        }
    }
    let mut lines = Vec::new();
    walk(env.temp_dir.path(), &mut lines);
    lines
        .into_iter()
        .filter(|l| l.starts_with('*') && l.contains(needle))
        .collect()
}

/// Serializes the arms against each other: two windowed apps must not be alive
/// in one process at the same time (see `undo_survives_blur_windowed.rs`).
static ONE_WINDOWED_APP_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// One arm. `loro` picks the storage mode; `suffix` keys the row ids so the
/// arms never share a `local_edit_epoch` entry.
fn drive_promote_then_blur(loro: bool, suffix: &str, window_title: &'static str) {
    let _serialized = ONE_WINDOWED_APP_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

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
        "this arm must boot the storage mode it claims — the blur commit is dropped in one mode \
         and is the sole content writer in the other"
    );
    let (edit_id, blur_id) = futures::executor::block_on(graft_undo_blur_pair(&env, suffix))
        .expect("graft the promote/blur row pair");

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
            "[keyword-blur-boot] rows not both painted yet; {} elements",
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

    assert_eq!(
        stored_pair(&env, &edit_id),
        (UNDO_EDIT_CONTENT.to_string(), None),
        "precondition: the grafted row starts plain and task-less"
    );

    // ── promote through a REAL click + REAL keystrokes ────────────────────
    futures::executor::block_on(driver.click_entity(&edit_target, "main"))
        .expect("click the edit target row's painted text");
    settle(&mut app, &bounds, Duration::from_secs(30));
    futures::executor::block_on(driver.send_raw_keystroke("home", &[]))
        .expect("caret to the head of the row");
    for key in ["T", "O", "D", "O", "space"] {
        futures::executor::block_on(driver.send_raw_keystroke(key, &[]))
            .unwrap_or_else(|e| panic!("typing {key:?} into the edit target row: {e}"));
    }
    settle(&mut app, &bounds, Duration::from_secs(30));

    // The promotion itself must have landed, else the blur below has no
    // promoted session to corrupt and every assertion after it is vacuous.
    assert_eq!(
        stored_pair(&env, &edit_id),
        (UNDO_EDIT_CONTENT.to_string(), Some("TODO".to_string())),
        "precondition: typing `TODO ` at the head must promote — stripped content in `content`, \
         the keyword in `task_state`"
    );

    let history_before = history_rows(&env, &edit_id);

    // ── the blur: click a DIFFERENT row ──────────────────────────────────
    futures::executor::block_on(driver.click_entity(&blur_target, "main"))
        .expect("click the sibling row to blur the promoted one");
    settle(&mut app, &bounds, Duration::from_secs(30));

    // NON-VACUITY 1: focus genuinely left the promoted row, so its blur edge ran.
    assert_eq!(
        engine.focused_block().as_ref(),
        Some(&blur_target),
        "the blur click must actually move focus off the promoted row — otherwise no blur commit \
         ran and the assertions below prove nothing"
    );

    let (content, task_state) = stored_pair(&env, &edit_id);
    let painted_now = painted_texts(&bounds, &edit_element);
    let headlines = org_headlines(&env, UNDO_EDIT_CONTENT);

    assert_eq!(
        content, UNDO_EDIT_CONTENT,
        "#99: the blur commit must not write the SURFACE (vault syntax) through the CONTENT \
         channel. The store now carries content={content:?} with task_state={task_state:?}, which \
         renders `* TODO TODO …` and re-ingests with the keyword in the title. Row paints \
         {painted_now:?}; org headlines {headlines:?}"
    );
    assert_eq!(
        task_state.as_deref(),
        Some("TODO"),
        "the blur must not lose the task state either — the block stays a task"
    );
    assert!(
        headlines.iter().all(|l| !l.contains("TODO TODO")),
        "the write-back must render exactly ONE keyword; disk headlines {headlines:?}"
    );

    // NON-VACUITY 2 (SqlOnly only): the blur ACTUALLY dispatched a write. In
    // SqlOnly the blur `set_field` is the editor's own commit funnel, so a blur
    // that emitted nothing would make the assertions above pass without ever
    // crossing the funnel under test. Under Loro the blur write is deliberately
    // dropped (a per-keystroke cell writer already committed), so the guard
    // would assert the opposite fact and is not applied to that arm.
    let history_after = history_rows(&env, &edit_id);
    if !loro {
        assert!(
            history_after > history_before,
            "vacuity guard: the blur dispatched NO operation ({history_before} history rows \
             before and after), so this arm never exercised the blur commit funnel"
        );
    }
    eprintln!(
        "[keyword-blur] loro={loro} history {history_before}->{history_after} \
         content={content:?} task_state={task_state:?} painted={painted_now:?}"
    );

    drop(driver);
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
}

/// THE ARM THE FINDING IS ABOUT: the shipped default, where the blur
/// `set_field` is the editor's own commit funnel.
#[test]
fn promoted_row_keeps_its_keyword_out_of_the_title_across_a_blur_sqlonly() {
    drive_promote_then_blur(false, "kwblur-sqlonly", "Holon-TestPlatform-KwBlur-SqlOnly");
}

/// The control: with a Loro cell attached the blur `set_field("content")` is
/// dropped as a redundant second writer, so this arm is expected to have been
/// green all along. It fails only if the fix breaks the drop.
#[test]
fn promoted_row_keeps_its_keyword_out_of_the_title_across_a_blur_loro() {
    drive_promote_then_blur(true, "kwblur-loro", "Holon-TestPlatform-KwBlur-Loro");
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
