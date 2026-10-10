//! A file adopts a block it holds a copy of while the copy and Holon's
//! version were edited apart: the adoption keeps one side, and the side it
//! drops is saved as a conflict copy that the disclosure names.
//!
//! * The user deletes the owner's version (from its file, or the whole file):
//!   the copy wins, and Holon's version is saved.
//! * Holon moves the block into the copy's page: Holon's version wins, and the
//!   file is saved.
//!
//! @pbt kind harness
//! @pbt covers cross-doc-membership(adoption) — the side an adoption drops
//! survives as a conflict copy
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone does not edit
//! a copy and its owner apart before the release

use std::sync::Arc;
use std::time::Duration;

use holon_integration_tests::TestEnvironment;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

const COPIED: &str = "block:bulk-0-0";

const DAY_PAGE: &str = "\
#+TITLE: DayPage
#+ID: area-daypage
* Q1W  q9
:PROPERTIES:
:ID: bulk-0-0
:END:
* Day keeps this
:PROPERTIES:
:ID: day-keeps
:END:
";

const DAY_PAGE_RELEASED: &str = "\
#+TITLE: DayPage
#+ID: area-daypage
* Day keeps this
:PROPERTIES:
:ID: day-keeps
:END:
";

const OVERVIEW: &str = "\
#+TITLE: Overview
#+ID: overview
* Q1W  q9
:PROPERTIES:
:ID: bulk-0-0
:END:
* Overview own line
:PROPERTIES:
:ID: overview-own
:END:
";

const OVERVIEW_TODO: &str = "\
#+TITLE: Overview
#+ID: overview
* TODO Q1W  q9
:PROPERTIES:
:ID: bulk-0-0
:END:
* Overview own line
:PROPERTIES:
:ID: overview-own
:END:
";

async fn read_file(env: &TestEnvironment, path: &std::path::Path) -> String {
    holon_filesystem::FileSystem::read_to_string(env.org_fs.as_ref(), path)
        .await
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

async fn rows(env: &TestEnvironment, id: &str, expr: &str) -> Vec<String> {
    env.query_sql(&format!(
        "SELECT {expr} AS v FROM block_raw WHERE id = '{id}'"
    ))
    .await
    .unwrap_or_else(|e| panic!("query {expr} of {id}: {e:#}"))
    .into_iter()
    .map(|r| match r.get("v") {
        Some(holon_api::Value::String(s)) => s.clone(),
        other => format!("{other:?}"),
    })
    .collect()
}

async fn wait_for_row(env: &TestEnvironment, id: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while rows(env, id, "id").await.is_empty() {
        assert!(std::time::Instant::now() < deadline, "{id} never landed");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
}

async fn settle(env: &TestEnvironment) {
    let seq = env.org_fs.last_change_seq();
    env.wait_for_org_change_processed(seq, Duration::from_secs(20))
        .await;
    tokio::time::sleep(Duration::from_millis(800)).await;
}

async fn save_and_settle(env: &TestEnvironment, name: &str, body: &str) {
    env.write_org_file(name, body)
        .await
        .unwrap_or_else(|e| panic!("write {name}: {e:#}"));
    settle(env).await;
}

async fn eventually_on_disk(env: &TestEnvironment, name: &str, needle: &str) {
    let path = env.org_root().join(name);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let disk = read_file(env, &path).await;
        if disk.contains(needle) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{name} never held {needle:?}:\n{disk}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Every conflict copy in the vault, with its text.
async fn conflict_copies(env: &TestEnvironment) -> Vec<(std::path::PathBuf, String)> {
    let scanned = holon_filesystem::FileSystem::scan_directory(env.org_fs.as_ref(), env.org_root())
        .await
        .expect("scan the vault");
    let mut copies = Vec::new();
    for path in scanned.files {
        if holon_core::conflict_copy::is_conflict_copy(&path) {
            let text = read_file(env, &path).await;
            copies.push((path, text));
        }
    }
    copies.sort();
    copies
}

/// The `Debug` text of every current condition.
fn conditions(env: &TestEnvironment) -> Vec<String> {
    env.injector()
        .expect("injector")
        .resolve::<Arc<holon_api::ConditionBus>>()
        .current()
        .into_iter()
        .map(|c| format!("{c:?}"))
        .collect()
}

async fn set_task_state(env: &TestEnvironment, state: &str) {
    let mut params: holon_api::StorageEntity = std::collections::HashMap::new();
    params.insert("id".into(), holon_api::Value::String(COPIED.to_string()));
    params.insert(
        "field".into(),
        holon_api::Value::String("task_state".into()),
    );
    params.insert("value".into(), holon_api::Value::String(state.into()));
    env.test_ctx()
        .execute_op("block", "set_field", params)
        .await
        .expect("set the copied block's task state in Holon");
}

/// DayPage.org owns the block, Overview.org holds a copy; the copy is set
/// TODO in the file and the block DONE in Holon.
async fn edited_apart(rt: Arc<tokio::runtime::Runtime>) -> TestEnvironment {
    let env = TestEnvironment::new(rt).expect("TestEnvironment::new");
    env.set_enable_loro(false);
    env.write_org_file("DayPage.org", DAY_PAGE)
        .await
        .expect("write DayPage.org");
    env.start_app(true).await.expect("start_app");
    wait_for_row(&env, COPIED).await;
    save_and_settle(&env, "Overview.org", OVERVIEW).await;
    save_and_settle(&env, "Overview.org", OVERVIEW_TODO).await;
    set_task_state(&env, "DONE").await;
    eventually_on_disk(&env, "DayPage.org", "DONE Q1W").await;
    assert!(
        conflict_copies(&env).await.is_empty(),
        "premise: no conflict copy yet"
    );
    env
}

/// The one conflict copy holds `needle`, and a condition of `kind` names it.
async fn assert_one_copy_holding(env: &TestEnvironment, needle: &str, kind: &str) {
    let copies = conflict_copies(env).await;
    assert_eq!(
        copies.len(),
        1,
        "the adoption must save the side it drops as one conflict copy: {copies:?}"
    );
    let (copy, text) = &copies[0];
    assert!(
        text.contains(needle) && text.contains(":ID: bulk-0-0"),
        "the conflict copy must hold the dropped {needle:?}:\n{text}"
    );
    let named: Vec<String> = conditions(env);
    assert!(
        named
            .iter()
            .any(|c| c.contains(kind) && c.contains(&copy.display().to_string())),
        "a {kind} condition must name the conflict copy {}: {named:?}",
        copy.display()
    );
}

/// The user deletes the block from its owner's file after the refused
/// release was disclosed: the copy's TODO wins, Holon's DONE is saved.
#[test]
fn a_repeated_release_saves_holons_version() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = edited_apart(rt).await;
        save_and_settle(&env, "DayPage.org", DAY_PAGE_RELEASED).await;
        assert_eq!(
            rows(&env, COPIED, "parent_id").await,
            vec!["block:area-daypage".to_string()],
            "premise: the first release is refused"
        );
        save_and_settle(&env, "DayPage.org", DAY_PAGE_RELEASED).await;
        assert_eq!(
            rows(&env, COPIED, "parent_id").await,
            vec!["block:overview".to_string()],
            "premise: the second release is adopted"
        );
        assert_eq!(
            rows(&env, COPIED, "json_extract(properties, '$.task_state')").await,
            vec!["TODO".to_string()],
            "premise: the copy wins"
        );
        assert_one_copy_holding(&env, "DONE Q1W", "HolonEditOverruled").await;
        env.stop_app().await.expect("stop_app");
    });
}

/// The user deletes the owner's whole file: the copy's TODO wins, Holon's
/// DONE is saved.
#[test]
fn deleting_the_owners_file_saves_holons_version() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = edited_apart(rt).await;
        holon_filesystem::FileSystem::remove(
            env.org_fs.as_ref(),
            &env.org_root().join("DayPage.org"),
        )
        .await
        .expect("delete DayPage.org");
        settle(&env).await;
        assert_eq!(
            rows(&env, COPIED, "parent_id").await,
            vec!["block:overview".to_string()],
            "premise: Overview.org adopts the block"
        );
        assert_eq!(
            rows(&env, COPIED, "json_extract(properties, '$.task_state')").await,
            vec!["TODO".to_string()],
            "premise: the copy wins"
        );
        assert_one_copy_holding(&env, "DONE Q1W", "HolonEditOverruled").await;
        env.stop_app().await.expect("stop_app");
    });
}

/// Holon moves the block into Overview's page: Holon's DONE wins, the file's
/// TODO is saved.
#[test]
fn moving_the_block_into_the_copys_page_saves_the_file() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = edited_apart(rt).await;
        let mut params = std::collections::HashMap::new();
        params.insert(
            "id".to_string(),
            holon_api::Value::String(COPIED.to_string()),
        );
        params.insert(
            "parent_id".to_string(),
            holon_api::Value::String("block:overview".to_string()),
        );
        env.execute_operation("block", "move_block", params)
            .await
            .expect("move the block in Holon into Overview's page");
        settle(&env).await;
        assert_eq!(
            rows(&env, COPIED, "parent_id").await,
            vec!["block:overview".to_string()],
            "premise: the block is in Overview's page"
        );
        assert_eq!(
            rows(&env, COPIED, "json_extract(properties, '$.task_state')").await,
            vec!["DONE".to_string()],
            "premise: Holon's version wins"
        );
        eventually_on_disk(&env, "Overview.org", "DONE Q1W").await;
        assert_one_copy_holding(&env, "TODO Q1W", "FileEditOverruled").await;
        env.stop_app().await.expect("stop_app");
    });
}
