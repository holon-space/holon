//! End-to-end: a `holon_sql` query using `fts_match` over block content
//! returns matching rows through the real backend engine.
//!
//! Two rungs (see docs/Proposals/FtsRegistry-2026-07-11.md):
//! 1. `fts_query_block_live_path` — the FULL holon_sql query-block route
//!    (`compile_to_sql` → `query_and_watch`, which materializes the query as a
//!    matview). This is what a user's `holon_sql` source block does.
//! 2. `fts_direct_query_over_block_content` — the same query via a direct
//!    one-shot read, isolating engine-level FTS from the IVM/matview layer.
//!
//! @pbt kind harness
//! @pbt covers fts-query-block — fts_match over block content through the real
//! engine

#![cfg(feature = "test-infra")]

use std::collections::HashMap;

use holon::di::test_helpers::create_test_engine;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::QueryLanguage;
use tokio_stream::StreamExt;

const FTS_QUERY: &str =
    "SELECT id, content FROM block_raw WHERE fts_match(content, 'tantivy') ORDER BY id";

async fn seed_fts_blocks(engine: &holon::api::backend_engine::BackendEngine) {
    engine
        .db_handle()
        .execute_ddl(&format!(
            "CREATE INDEX fts_block_content ON {BLOCK_WRITE_TABLE} USING fts (content)"
        ))
        .await
        .expect("create FTS index over block content");

    let sql = format!(
        "INSERT INTO {BLOCK_WRITE_TABLE} (id, parent_id, content, content_type) VALUES \
         ('block:f1', NULL, 'notes on the tantivy search engine', 'text'), ('block:f2', NULL, \
         'grocery list milk bread', 'text'), ('block:f3', NULL, 'tantivy segments are immutable', \
         'text')"
    );
    engine
        .db_handle()
        .execute(&sql, vec![])
        .await
        .expect("seed blocks");
}

#[tokio::test(flavor = "multi_thread")]
async fn fts_direct_query_over_block_content() {
    let engine = create_test_engine().await.expect("engine");
    seed_fts_blocks(&engine).await;

    let rows = engine
        .db_handle()
        .query_positional(FTS_QUERY, vec![])
        .await
        .expect("fts_match query over block content");

    let ids: Vec<String> = rows
        .iter()
        .map(|r| match r.get("id") {
            Some(holon_api::Value::String(t)) => t.to_string(),
            other => panic!("expected text id, got {other:?}"),
        })
        .collect();
    assert_eq!(ids, vec!["block:f1", "block:f3"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn fts_query_block_live_path() {
    let engine = create_test_engine().await.expect("engine");
    seed_fts_blocks(&engine).await;

    // Exactly what a holon_sql source block does: pass-through compile, then
    // the live query_and_watch route (matview-backed).
    let sql = engine
        .compile_to_sql(FTS_QUERY, QueryLanguage::HolonSql)
        .expect("holon_sql compile is pass-through");
    let stream = engine
        .query_and_watch(sql, HashMap::new(), None)
        .await
        .expect("query_and_watch for fts_match holon_sql query");
    tokio::pin!(stream);

    let initial = tokio::time::timeout(std::time::Duration::from_secs(10), stream.next())
        .await
        .expect("initial batch within 10s")
        .expect("stream should not close");

    let mut ids: Vec<String> = initial
        .inner
        .items
        .iter()
        .filter_map(|c| match &c.change {
            holon_api::streaming::Change::Created { data, .. } => {
                data.get("id").and_then(|v| v.as_string()).map(String::from)
            }
            _ => None,
        })
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["block:f1", "block:f3"]);
}
