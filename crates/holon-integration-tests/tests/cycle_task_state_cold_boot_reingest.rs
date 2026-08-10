//! F3, the whole loop: cycling a task in a document that declares its own
//! `#+TODO:` vocabulary must survive a COLD BOOT re-ingest.
//!
//! The bug is not the wrong keyword — it is the SILENT DELETION that follows.
//! `cycle_task_state` writing `TODO` into a `#+TODO: NEXT WAITING | DONE`
//! document renders `* TODO delta external`; the org parser of that document
//! does not know `TODO`, so the next full parse reads the headline as PLAIN
//! BODY TEXT with content `"TODO delta external"` and no `task_state`. The task
//! is gone, and nothing anywhere reported an error.
//!
//! WHY A COLD BOOT AND NOT AN EDIT. An in-place re-ingest only re-parses files
//! whose bytes CHANGED, and the changed-file path reconciles against the store
//! it already has — so it can preserve a `task_state` the parser alone would
//! not derive, and the test FALSE-PASSES. The second boot here runs against a
//! FRESH DATABASE with nothing but the rendered bytes, which is the only
//! configuration where the parser is the sole authority.
//!
//! @pbt kind harness
//! @pbt covers task-state-cycle-vocabulary — a cycled keyword survives a
//!   fresh-DB re-ingest of the document that declares it

use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::Value;
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

const VOCAB_ORG: &str = "\
#+TITLE: Errands
#+ID: page-errands
#+TODO: NEXT WAITING | DONE

* delta external
:PROPERTIES:
:ID: blk-delta
:END:
";

const BLOCK_ID: &str = "block:blk-delta";

/// The `(file name, bytes)` of the org file that carries the page — found by
/// its `#+ID:` rather than by name, because the write-back is free to rename
/// the file after the page's title.
async fn rendered_page(env: &TestEnvironment) -> (String, String) {
    use holon_filesystem::FileSystem;

    let mut hits: Vec<(String, String)> = Vec::new();
    let mut targets = env.org_fs.write_targets();
    targets.sort();
    targets.dedup();
    for path in targets {
        if path.extension().and_then(|e| e.to_str()) != Some("org") {
            continue;
        }
        let Ok(body) = env.org_fs.read_to_string(&path).await else {
            continue;
        };
        if !body.contains("page-errands") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("utf-8 file name")
            .to_string();
        hits.push((name, body));
    }
    assert_eq!(
        hits.len(),
        1,
        "expected exactly one org file rooting page-errands, found {:?}",
        hits.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
    hits.remove(0)
}

async fn wait_for_seed(env: &TestEnvironment) {
    let deadline = std::time::Instant::now() + Duration::from_secs(25);
    loop {
        let rows = env
            .query_sql(&format!("SELECT id FROM block_raw WHERE id = '{BLOCK_ID}'"))
            .await
            .expect("query block_raw");
        if !rows.is_empty() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "org scan never populated {BLOCK_ID}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn settle(env: &TestEnvironment) {
    env.wait_for_loro_quiescence(Duration::from_secs(15)).await;
    env.wait_for_cdc_quiescent(Duration::from_millis(250), Duration::from_secs(15))
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
}

/// `(content, task_state)` of the block, straight out of `block_raw`.
async fn block_row(env: &TestEnvironment) -> (String, Option<String>) {
    let rows = env
        .query_sql(&format!(
            "SELECT content, json_extract(properties, '$.task_state') AS ts FROM block_raw WHERE \
             id = '{BLOCK_ID}'"
        ))
        .await
        .expect("query block_raw");
    let row = rows
        .first()
        .unwrap_or_else(|| panic!("{BLOCK_ID} missing from block_raw"));
    let content = row
        .get("content")
        .and_then(|v| v.as_string())
        .unwrap_or_default()
        .to_string();
    let task_state = row
        .get("ts")
        .and_then(|v| v.as_string())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    (content, task_state)
}

#[test]
fn a_cycled_keyword_survives_a_fresh_db_reingest_of_its_own_document() {
    let rt = runtime();
    rt.clone().block_on(async move {
        // ── Boot 1: seed the vault, cycle the child once ────────────────
        let mut env = TestEnvironment::new(rt.clone()).expect("TestEnvironment::new");
        env.write_org_file("errands.org", VOCAB_ORG)
            .await
            .expect("write errands.org");
        env.start_app(true).await.expect("boot-1 start_app");
        wait_for_seed(&env).await;
        settle(&env).await;

        // Nothing seeds the vocabulary: the `#+TODO:` header on disk is its
        // only source, exactly as in the field. The premise assertion below
        // (`NEXT`, not `TODO`) is what proves the ingest delivered it.
        let mut params = holon_api::StorageEntity::new();
        params.insert("id".into(), Value::String(BLOCK_ID.into()));
        env.engine()
            .execute_operation(
                &EntityName::new("block"),
                "cycle_task_state",
                params,
                OpOrigin::User,
            )
            .await
            .expect("cycle_task_state");
        settle(&env).await;

        let (content_1, state_1) = block_row(&env).await;
        assert_eq!(
            state_1.as_deref(),
            Some("NEXT"),
            "boot-1 premise: the cycle must land on a keyword THIS document \
             declares (content = {content_1:?})"
        );

        // The rendered projection is the only thing that crosses to boot 2.
        let (rendered_name, rendered) = rendered_page(&env).await;
        eprintln!("[cold-boot] rendered {rendered_name}:\n{rendered}");
        assert!(
            rendered.contains("#+TODO:"),
            "the rendered file lost its vocabulary declaration, so boot 2 would judge the \
             keyword against the defaults and this test would prove nothing:\n{rendered}"
        );
        env.stop_app().await.expect("stop_app after boot-1");

        // ── Boot 2: a FRESH database over those bytes ───────────────────
        let mut cold = TestEnvironment::new(rt).expect("cold TestEnvironment::new");
        cold.write_org_file(&rendered_name, &rendered)
            .await
            .expect("seed the cold vault");
        cold.start_app(true).await.expect("boot-2 start_app");
        wait_for_seed(&cold).await;
        settle(&cold).await;

        let (content_2, state_2) = block_row(&cold).await;
        cold.stop_app().await.expect("stop_app after boot-2");

        assert_eq!(
            state_2.as_deref(),
            Some("NEXT"),
            "the cycled task VANISHED across a fresh-DB re-ingest: the keyword written by \
             `cycle_task_state` is not one this document declares, so the parser read the \
             headline back as body text (content = {content_2:?})"
        );
        assert_eq!(
            content_2, "delta external",
            "the keyword must not be sitting inside the block's text"
        );
    });
}
