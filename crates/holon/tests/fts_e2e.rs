//! FTS maintenance-contract test: verifies the Turso fork's Tantivy-backed
//! `USING fts` index method is WRITE-MAINTAINED through holon's storage stack
//! (DatabaseActor / DatabaseHandle), not rebuild-only.
//!
//! Contract under test (documented in
//! docs/Proposals/FtsRegistry-2026-07-11.md):
//! - INSERT: newly inserted rows are immediately visible to fts_match
//! - UPDATE: the index reflects the new content; stale terms stop matching
//! - DELETE: deleted rows stop matching
//! - fts_score is usable as an ordering scalar over the same index

use holon::storage::test_helpers::create_test_backend_with_tempdir;
use holon_api::Value;

fn ids(rows: &[holon_api::StorageEntity]) -> Vec<String> {
    let mut out: Vec<String> = rows
        .iter()
        .map(|row| match row.get("id") {
            Some(Value::String(t)) => t.to_string(),
            other => panic!("expected text id column, got {other:?}"),
        })
        .collect();
    out.sort();
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn fts_index_is_write_maintained() {
    let (_tmp, backend) = create_test_backend_with_tempdir().await;
    let db = backend.handle();

    // Shape mirrors block content storage: text primary key + text content.
    db.execute(
        "CREATE TABLE fts_blocks (id TEXT PRIMARY KEY, content TEXT)",
        vec![],
    )
    .await
    .unwrap();

    db.execute_ddl("CREATE INDEX fts_blocks_content ON fts_blocks USING fts (content)")
        .await
        .unwrap();

    for (id, content) in [
        ("b1", "quarterly report about revenue growth"),
        ("b2", "meeting notes about the tantivy search engine"),
        ("b3", "grocery list milk bread eggs"),
    ] {
        db.execute(
            "INSERT INTO fts_blocks (id, content) VALUES (?, ?)",
            vec![
                turso::Value::Text(id.to_string()),
                turso::Value::Text(content.to_string()),
            ],
        )
        .await
        .unwrap();
    }

    // INSERT maintenance: rows inserted after index creation must match.
    let rows = db
        .query_positional(
            "SELECT id FROM fts_blocks WHERE fts_match(content, 'revenue')",
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(ids(&rows), vec!["b1"], "insert not indexed");

    let rows = db
        .query_positional(
            "SELECT id FROM fts_blocks WHERE fts_match(content, 'search')",
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(ids(&rows), vec!["b2"]);

    // UPDATE maintenance: new terms match, stale terms must not.
    db.execute(
        "UPDATE fts_blocks SET content = 'meeting notes about embeddings and vectors' WHERE id = \
         'b2'",
        vec![],
    )
    .await
    .unwrap();

    let rows = db
        .query_positional(
            "SELECT id FROM fts_blocks WHERE fts_match(content, 'embeddings')",
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(ids(&rows), vec!["b2"], "update not indexed");

    let rows = db
        .query_positional(
            "SELECT id FROM fts_blocks WHERE fts_match(content, 'search')",
            vec![],
        )
        .await
        .unwrap();
    assert!(
        rows.is_empty(),
        "stale term still matches after UPDATE — index not write-maintained: {rows:?}"
    );

    // DELETE maintenance.
    db.execute("DELETE FROM fts_blocks WHERE id = 'b1'", vec![])
        .await
        .unwrap();

    let rows = db
        .query_positional(
            "SELECT id FROM fts_blocks WHERE fts_match(content, 'revenue')",
            vec![],
        )
        .await
        .unwrap();
    assert!(
        rows.is_empty(),
        "deleted row still matches — index not write-maintained: {rows:?}"
    );

    // fts_score usable as a scalar with ORDER BY over the maintained index.
    let rows = db
        .query_positional(
            "SELECT fts_score(content, 'meeting notes') AS score, id FROM fts_blocks WHERE \
             fts_match(content, 'meeting notes') ORDER BY score DESC",
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(ids(&rows), vec!["b2"]);
    match rows[0].get("score") {
        Some(Value::Float(f)) => assert!(*f > 0.0, "score should be positive, got {f}"),
        other => panic!("expected float score, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn fts_functions_resolve_without_index() {
    // fts_highlight is a standalone scalar (no index needed) — proves the Func
    // enum path resolves the fts_* family in holon builds with the feature on.
    let (_tmp, backend) = create_test_backend_with_tempdir().await;
    let db = backend.handle();

    let rows = db
        .query_positional(
            "SELECT fts_highlight('learn about database optimization', '<b>', '</b>', 'database') \
             AS h",
            vec![],
        )
        .await
        .unwrap();
    match rows[0].get("h") {
        Some(Value::String(t)) => {
            assert_eq!(t.as_str(), "learn about <b>database</b> optimization")
        }
        other => panic!("expected highlighted text, got {other:?}"),
    }
}
