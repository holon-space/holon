//! The windowed rung for dogfood finding F1 of the Cucumber rehearsal
//! (task #94): a structural chord must not flush a STALE editor buffer over a
//! store state that block's editor never authored.
//!
//! THE SHAPE (3/3 reproductions against a real app): click a row, type into it,
//! then split it from a NON-EDITOR origin (`execute_operation
//! block.split_block`, what an MCP client or a peer does) while that editor
//! still holds focus. The store is now correct — prefix in the original, suffix
//! in a freshly minted sibling — but the focused editor's visible buffer still
//! shows the PRE-SPLIT text. The next structural chord (Tab) treats itself as a
//! commit point and flushes that buffer first
//! (`EditorView::dispatch_structural_as_commit_point` →
//! `EditorViewModel::pending_commit_intent`), re-writing the pre-split text
//! over the truncated row. The suffix block survives, so the text is DUPLICATED
//! and persisted.
//!
//! WHY IT ESCAPED EVERY EXISTING RUNG: the headless keystone's editor mirror is
//! converged unconditionally by the harness settle, so it can never hold a
//! stale buffer; and no rung follows a non-editor-origin STRUCTURAL write with
//! a structural CHORD. The two single-variable controls from the dogfood name
//! the seam exactly: an MCP `set_field` under the same focused editor is NOT
//! reverted (its echo converges the editor), and a plain blur does not revert
//! either.
//!
//! MODE: SqlOnly (`crdt.enabled = false`) — the mode the dogfood ran. The Loro
//! arm is kept as a labelled control.
//!
//! Run: cargo test -p holon-gpui --features pbt --test
//! main structural_chord_stale_flush_windowed::

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use holon_api::EntityUri;
use holon_api::Value;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::user_driver::UserDriver;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::window_slice::seed::GRAFT_PAGE_ID;
use holon_integration_tests::pbt::window_slice::seed::UNDO_EDIT_CONTENT;
use holon_integration_tests::pbt::window_slice::seed::graft_undo_blur_pair;
use holon_integration_tests::test_environment::TestEnvironment;
use pbt_harness::sim_windowed_replay::SimUserDriver;

use crate::pbt_harness;
use crate::pbt_harness::settle::HANG_DEADLINE;
use crate::pbt_harness::settle::hold_tokio_workers;
use crate::pbt_harness::settle::settle_window;

/// The typed tail. Distinctive enough that a duplicate is unambiguous in a
/// whole-store scan, and keyword-free so no task facet is involved.
const TAIL: &str = "ZZZZ";

fn real_text_system() -> Arc<dyn gpui::PlatformTextSystem> {
    gpui_platform::current_text_system()
}

fn settle_frame(
    app: &HeadlessAppContext,
    bounds: &BoundsRegistry,
    runtime: &tokio::runtime::Runtime,
    env: &TestEnvironment,
) {
    let engine = env
        .reactive_engine
        .get()
        .expect("reactive engine after start_app");
    settle_window(
        app,
        bounds,
        engine.ui_watermark(),
        &mut || runtime.block_on(env.settle()),
        HANG_DEADLINE,
    )
    .unwrap_or_else(|e| panic!("window settle: {e}"));
}

/// Longer than a window settle of an idle window: the clicked editor's task
/// vocabulary resolves over a tokio hop, so it lands after a settle that does
/// not wait for it, as it does on a loaded host.
const TOKIO_HOLD: Duration = Duration::from_millis(300);

/// Click `target`, then settle. With `hold`, the engine settles first and every
/// tokio worker stays busy across the click, so the window settle alone must
/// wait until the clicked editor's task vocabulary has resolved.
fn click_and_settle(
    driver: &SimUserDriver,
    target: &EntityUri,
    hold: bool,
    app: &HeadlessAppContext,
    bounds: &BoundsRegistry,
    runtime: &tokio::runtime::Runtime,
    env: &TestEnvironment,
) {
    if !hold {
        futures::executor::block_on(driver.click_entity(target, "main"))
            .unwrap_or_else(|e| panic!("click {target}: {e:#}"));
        settle_frame(app, bounds, runtime, env);
        return;
    }
    runtime.block_on(env.settle());
    hold_tokio_workers(runtime, TOKIO_HOLD);
    futures::executor::block_on(driver.click_entity(target, "main"))
        .unwrap_or_else(|e| panic!("click {target} with the tokio workers held: {e:#}"));
    let engine = env
        .reactive_engine
        .get()
        .expect("reactive engine after start_app");
    settle_window(
        app,
        bounds,
        engine.ui_watermark(),
        &mut || {},
        HANG_DEADLINE,
    )
    .unwrap_or_else(|e| panic!("window settle with the tokio workers held: {e}"));
}

fn sql_rows(env: &TestEnvironment, sql: String) -> Vec<HashMap<Arc<str>, Value>> {
    futures::executor::block_on(async {
        env.engine()
            .execute_query(sql, HashMap::new(), None)
            .await
            .expect("query the store")
    })
}

fn string_col(row: &HashMap<Arc<str>, Value>, col: &str) -> Option<String> {
    match row.get(col) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// `content` of one block, straight from `block_raw`.
fn stored_content(env: &TestEnvironment, id: &str) -> String {
    let uri = EntityUri::block(id).to_string();
    let rows = sql_rows(
        env,
        format!("SELECT content FROM block_raw WHERE id = '{uri}'"),
    );
    let row = rows.first().expect("the edited block must exist");
    string_col(row, "content").expect("block_raw content is a string")
}

/// Every block in the store whose content mentions the tail, as
/// `(id, content, parent_id)`. The duplication signature is this list having
/// more than one entry.
fn blocks_mentioning_tail(env: &TestEnvironment) -> Vec<(String, String, String)> {
    let mut rows: Vec<(String, String, String)> = sql_rows(
        env,
        format!("SELECT id, content, parent_id FROM block_raw WHERE content LIKE '%{TAIL}%'"),
    )
    .iter()
    .map(|r| {
        (
            string_col(r, "id").unwrap_or_default(),
            string_col(r, "content").unwrap_or_default(),
            string_col(r, "parent_id").unwrap_or_default(),
        )
    })
    .collect();
    rows.sort();
    rows
}

/// Every org line under the vault root that mentions the tail — the disk side
/// of the duplication, asserted where it lands rather than inferred.
fn org_lines_mentioning_tail(env: &TestEnvironment) -> Vec<String> {
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
    lines.retain(|l| l.contains(TAIL));
    lines
}

/// `(id, parent_id)` for every block under the Main focus root, so a structural
/// chord's effect on the tree is observable as a vacuity guard.
fn parentage(env: &TestEnvironment) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = sql_rows(
        env,
        "SELECT id, parent_id FROM block_raw WHERE parent_id IS NOT NULL".to_string(),
    )
    .iter()
    .map(|r| {
        (
            string_col(r, "id").unwrap_or_default(),
            string_col(r, "parent_id").unwrap_or_default(),
        )
    })
    .collect();
    rows.sort();
    rows
}

/// Serializes the arms against each other: two windowed apps must not be alive
/// in one process at the same time (see `task_keyword_blur_windowed.rs`).
static ONE_WINDOWED_APP_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// One arm. `loro` picks the storage mode; `suffix` keys the row ids so the
/// arms never share a `local_edit_epoch` entry.
fn drive_external_split_then_chord(
    loro: bool,
    hold: bool,
    suffix: &str,
    window_title: &'static str,
) {
    let _serialized = ONE_WINDOWED_APP_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let _reactor = runtime.enter();
    let env = futures::executor::block_on(async { TestEnvironment::new(runtime.clone()).unwrap() });
    // Mode BEFORE boot: the wiring is read once at `start_app`.
    env.set_enable_loro(loro);
    futures::executor::block_on(async { env.start_app(true).await.expect("start_app") });
    assert_eq!(
        env.loro_enabled(),
        loro,
        "this arm must boot the storage mode it claims"
    );
    let (edit_id, sibling_id) = futures::executor::block_on(graft_undo_blur_pair(&env, suffix))
        .expect("graft the edit/sibling row pair");
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
    settle_frame(&app, &bounds, &runtime, &env);
    let painted = bounds
        .all_elements()
        .iter()
        .any(|(_, info)| info.entity_id.as_deref() == Some(edit_element.as_str()));
    assert!(
        painted,
        "boot precondition: the edit row must paint before anything is typed"
    );

    // The pair is grafted in order, so the edit row is the FIRST child and Tab
    // has no previous sibling to indent it under. Move the sibling ahead of it
    // after boot has painted the edit row: under Loro a move issued right after
    // the create fails with "Block not found".
    let mut params: HashMap<String, Value> = HashMap::new();
    params.insert(
        "id".into(),
        Value::String(EntityUri::block(&sibling_id).to_string()),
    );
    params.insert(
        "parent_id".into(),
        Value::String(EntityUri::block(GRAFT_PAGE_ID).to_string()),
    );
    futures::executor::block_on(env.execute_operation("block", "move_block", params))
        .expect("move the sibling ahead of the edit row");
    settle_frame(&app, &bounds, &runtime, &env);

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
    let edit_target = EntityUri::from_raw(&edit_id);

    assert_eq!(
        stored_content(&env, &edit_id),
        UNDO_EDIT_CONTENT,
        "precondition: the grafted row starts at its seeded content"
    );

    // ── type the tail through a REAL click + REAL keystrokes ─────────────
    // The editor must be the row's last writer, so the split below is a
    // genuinely EXTERNAL write against a buffer this editor authored.
    click_and_settle(&driver, &edit_target, hold, &app, &bounds, &runtime, &env);
    futures::executor::block_on(driver.send_raw_keystroke("end", &[]))
        .expect("caret to the end of the row");
    for key in ["Z", "Z", "Z", "Z"] {
        futures::executor::block_on(driver.send_raw_keystroke(key, &[]))
            .unwrap_or_else(|e| panic!("typing {key:?} into the edit target row: {e}"));
    }
    settle_frame(&app, &bounds, &runtime, &env);

    let typed = format!("{UNDO_EDIT_CONTENT}{TAIL}");
    assert_eq!(
        stored_content(&env, &edit_id),
        typed,
        "precondition: the typed tail must land in the store, else the split below is not \
         splitting text this editor authored"
    );

    // ── the NON-EDITOR-ORIGIN structural write ───────────────────────────
    // Exactly what the dogfood did over MCP: split the focused row from
    // outside its editor. `position` is the byte offset of the tail.
    let split_at = UNDO_EDIT_CONTENT.len() as i64;
    let mut params: HashMap<String, Value> = HashMap::new();
    params.insert("id".into(), Value::String(edit_target.to_string()));
    params.insert("position".into(), Value::Integer(split_at));
    futures::executor::block_on(env.execute_operation("block", "split_block", params))
        .expect("external split_block");
    settle_frame(&app, &bounds, &runtime, &env);

    // PRECONDITION: the split really landed and really is external — the
    // original truncated, the tail in a NEW block.
    assert_eq!(
        stored_content(&env, &edit_id),
        UNDO_EDIT_CONTENT,
        "precondition: the external split must truncate the original row"
    );
    let after_split = blocks_mentioning_tail(&env);
    assert_eq!(
        after_split.len(),
        1,
        "precondition: exactly one block must carry the tail after the split; got {after_split:?}"
    );
    let parents_before_chord = parentage(&env);

    // ── the structural chord ─────────────────────────────────────────────
    futures::executor::block_on(driver.send_raw_keystroke("tab", &[]))
        .expect("the structural chord must be consumed by the window");
    settle_frame(&app, &bounds, &runtime, &env);

    let after_chord = blocks_mentioning_tail(&env);
    let edit_content = stored_content(&env, &edit_id);
    let disk = org_lines_mentioning_tail(&env);
    let parents_after_chord = parentage(&env);

    eprintln!(
        "[chord-flush] loro={loro} edit_content={edit_content:?} tail_blocks={after_chord:?} \
         disk={disk:?}"
    );

    // NON-VACUITY: the chord actually reached the store as a structural op.
    // Without this, "no duplication" could mean "the chord did nothing".
    assert_ne!(
        parents_before_chord, parents_after_chord,
        "vacuity guard: the Tab chord changed no parentage, so no structural op was dispatched \
         and the assertions below prove nothing"
    );

    // THE FINDING.
    assert_eq!(
        edit_content, UNDO_EDIT_CONTENT,
        "#94: the structural chord flushed the focused editor's STALE pre-split buffer over the \
         truncated row. content={edit_content:?} (expected {UNDO_EDIT_CONTENT:?}); blocks \
         carrying the tail: {after_chord:?}; org lines: {disk:?}"
    );
    assert_eq!(
        after_chord.len(),
        1,
        "#94: the tail text now exists in {} blocks — the chord's pre-flush re-wrote the pre-split \
         text while the split's tail block survived. Blocks: {after_chord:?}; org lines: {disk:?}",
        after_chord.len()
    );
    // DISCLOSED GAP — the disk half of the finding is NOT asserted here.
    // Write-back never runs for these rows in this harness: the grafted pair
    // makes the journal document's held membership disagree with the
    // authority's, and `FileSyncController` correctly SKIPS the render rather
    // than project a half-folded document ("write-back SKIPPED: the holder's
    // membership does not match the authority's" in every run). So `disk` is
    // always empty and any assertion over it would pass vacuously. The store
    // assertions above are the real gate; the dogfood established that org
    // write-back projects the store faithfully, so a duplicated store row is
    // what reaches disk.
    assert!(
        disk.is_empty(),
        "write-back now reaches these rows — replace this disclosed gap with a real disk \
         assertion (exactly one line carrying the tail). Lines: {disk:?}"
    );

    drop(driver);
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
}

/// THE ARM THE FINDING IS ABOUT: the shipped default.
#[test]
fn structural_chord_does_not_flush_a_stale_buffer_over_an_external_split_sqlonly() {
    drive_external_split_then_chord(
        false,
        false,
        "chordflush-sqlonly",
        "Holon-TestPlatform-ChordFlush-SqlOnly",
    );
}

/// The control arm: with a Loro cell attached the editor's content writes go
/// through the CRDT, so the pending-commit funnel is a no-op.
#[test]
fn structural_chord_does_not_flush_a_stale_buffer_over_an_external_split_loro() {
    drive_external_split_then_chord(
        true,
        false,
        "chordflush-loro",
        "Holon-TestPlatform-ChordFlush-Loro",
    );
}

/// The finding's arm on a loaded host: every tokio worker is busy across the
/// click, so the `end` keystroke after it lands only if the window settle
/// waits for the editor's task vocabulary.
#[test]
fn structural_chord_does_not_flush_a_stale_buffer_over_an_external_split_sqlonly_tokio_held() {
    drive_external_split_then_chord(
        false,
        true,
        "chordflush-sqlonly-held",
        "Holon-TestPlatform-ChordFlush-SqlOnly-Held",
    );
}

/// The Loro control on a loaded host.
#[test]
fn structural_chord_does_not_flush_a_stale_buffer_over_an_external_split_loro_tokio_held() {
    drive_external_split_then_chord(
        true,
        true,
        "chordflush-loro-held",
        "Holon-TestPlatform-ChordFlush-Loro-Held",
    );
}
