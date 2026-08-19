//! F3, the production path: a document's declared `#+TODO:` vocabulary must
//! reach the store on a REAL ingest, so the engine seam that reads it
//! (`SqlTaskVocabularySource`) resolves the DECLARED keywords and not the
//! built-in defaults.
//!
//! Every other vocabulary test hand-seeds `todo_keywords` through `set_field`.
//! A hand-seeded green proves the reader, never the writer — and the writer is
//! what the field runs. This test never touches `set_field`: the only thing
//! that ever declares a vocabulary here is the org file on disk.
//!
//! The assertion is taken at the SOURCE's own seam rather than at a row picked
//! by hand, because "which row carries it" is precisely what a hand-picked-row
//! test gets to assume and production does not.
//!
//! @pbt kind harness
//! @pbt covers task-state-cycle-vocabulary — an org-declared `#+TODO:` reaches
//!   the engine's vocabulary source through the real ingest

use std::sync::Arc;
use std::time::Duration;

use holon::api::task_vocabulary_source::SqlTaskVocabularySource;
use holon::core::task_keyword_promotion::TaskVocabularySource;
use holon::storage::BLOCK_WRITE_TABLE;
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

async fn wait_for_seed(env: &TestEnvironment) {
    let deadline = std::time::Instant::now() + Duration::from_secs(25);
    loop {
        let rows = env
            .query_sql(&format!(
                "SELECT id FROM {BLOCK_WRITE_TABLE} WHERE id = '{BLOCK_ID}'"
            ))
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

/// Every block row plus its `Page` tag count — the whole shape the vocabulary
/// walk sees, printed so a failure names the row that should have carried the
/// declaration.
async fn dump_rows(env: &TestEnvironment) -> String {
    let rows = env
        .query_sql(&format!(
            "SELECT b.id AS id, b.parent_id AS parent_id, b.content AS content, b.properties AS \
             properties, (SELECT COUNT(*) FROM block_tags t WHERE t.block_id = b.id AND t.tag = \
             'Page') AS page_tags FROM {BLOCK_WRITE_TABLE} b"
        ))
        .await
        .expect("dump block_raw");
    let mut out = String::new();
    for row in &rows {
        out.push_str(&format!("{row:?}\n"));
    }
    out
}

async fn task_state(env: &TestEnvironment) -> (String, Option<String>) {
    let rows = env
        .query_sql(&format!(
            "SELECT content, json_extract(properties, '$.task_state') AS ts FROM \
             {BLOCK_WRITE_TABLE} WHERE id = '{BLOCK_ID}'"
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
    let ts = row
        .get("ts")
        .and_then(|v| v.as_string())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    (content, ts)
}

/// Both storage modes, because the seam that carries the declaration differs
/// between them: under Loro authority the write must travel Loro-first or the
/// outbound projection reverts it, while SqlOnly writes the row directly.
fn declared_vocabulary_survives_ingest(loro: bool) {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        env.set_enable_loro(loro);
        env.write_org_file("errands.org", VOCAB_ORG)
            .await
            .expect("write errands.org");
        env.start_app(true).await.expect("start_app");
        wait_for_seed(&env).await;
        settle(&env).await;

        let dump = dump_rows(&env).await;
        eprintln!("[prod-path] block_raw after ingest:\n{dump}");
        // The seam the engine reads, built exactly as `BackendEngine` builds it.
        let source =
            SqlTaskVocabularySource::new(env.engine().db_handle().clone(), BLOCK_WRITE_TABLE);
        let vocab = source
            .vocabulary_for_block(BLOCK_ID)
            .await
            .expect("vocabulary_for_block");

        assert_eq!(
            vocab.active_keywords(),
            ["NEXT".to_string(), "WAITING".to_string()],
            "the document declares `#+TODO: NEXT WAITING | DONE`, but the engine's vocabulary \
             source resolved {vocab:?} — the declaration never reached the store.\nblock_raw:\n\
             {dump}"
        );
        assert_eq!(
            vocab.done_keywords(),
            ["DONE".to_string()],
            "declared done keyword missing from {vocab:?}\nblock_raw:\n{dump}"
        );

        // ...and the write seam that consumes it must obey the declaration.
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

        let (content, state) = task_state(&env).await;
        env.stop_app().await.expect("stop_app");

        assert_eq!(
            state.as_deref(),
            Some("NEXT"),
            "`cycle_task_state` must reach the first keyword THIS document declares, never \
             `TODO` — a keyword the document cannot parse back (content = {content:?})"
        );
    });
}

#[test]
fn an_org_declared_vocabulary_reaches_the_engines_vocabulary_source() {
    declared_vocabulary_survives_ingest(true);
}

#[test]
fn an_org_declared_vocabulary_reaches_the_engines_vocabulary_source_sql_only() {
    declared_vocabulary_survives_ingest(false);
}
