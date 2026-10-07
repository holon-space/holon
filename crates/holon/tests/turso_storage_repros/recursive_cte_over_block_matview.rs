//! A `WITH RECURSIVE` descendant walk over the chained `block` matview returns
//! every reachable row, as a direct query and as a matview, with and without
//! an `EXISTS` filter in the anchor member. Pins the engine side of entry
//! `2026-10-07-recursive-live-query-renders-only-roots`.

use std::collections::BTreeSet;
use std::collections::HashMap;

use holon::storage::turso::TursoBackend;
use tempfile::TempDir;

const SHAPES: &[(&str, &str)] = &[
    (
        "join_block",
        "WITH RECURSIVE d(id) AS (SELECT b.id FROM block b WHERE {P} UNION ALL SELECT c.id \
         FROM block c JOIN d ON c.parent_id = d.id) SELECT b.id FROM block b JOIN d ON d.id = b.id",
    ),
    (
        "select_cte",
        "WITH RECURSIVE d(id) AS (SELECT b.id FROM block b WHERE {P} UNION ALL SELECT c.id \
         FROM block c JOIN d ON c.parent_id = d.id) SELECT id FROM d",
    ),
];

const PLAIN: &str = "json_extract(b.properties, '$.task_state') = '?'";
const WITH_EXISTS: &str = "json_extract(b.properties, '$.task_state') = '?' AND EXISTS (SELECT \
                           1 FROM block_tags bt WHERE bt.block_id = b.id AND bt.tag = 'decision')";

fn ids(rows: &[holon_api::StorageEntity]) -> BTreeSet<String> {
    rows.iter()
        .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(String::from))
        .collect()
}

#[tokio::test]
async fn recursive_cte_reaches_every_descendant() {
    let temp = TempDir::new().unwrap();
    let db = TursoBackend::open_database(&temp.path().join("p.db")).expect("open db");
    let (cdc_tx, _cdc_rx) = tokio::sync::broadcast::channel(4096);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");
    handle
        .execute_ddl(
            "CREATE TABLE block_raw (id TEXT PRIMARY KEY, parent_id TEXT DEFAULT '', properties \
             TEXT)",
        )
        .await
        .unwrap();
    handle
        .execute_ddl("CREATE TABLE block_tags (block_id TEXT, tag TEXT)")
        .await
        .unwrap();
    handle
        .execute(
            "INSERT INTO block_raw (id, parent_id, properties) VALUES ('page', '', '{}'), \
             ('q1', 'page', '{\"task_state\":\"?\"}'), ('q2', 'page', '{\"task_state\":\"?\"}'), \
             ('other', 'page', '{}'), ('q1a', 'q1', '{}'), ('q1b', 'q1', '{}'), \
             ('q1a1', 'q1a', '{}'), ('q2a', 'q2', '{\"task_state\":\"TODO\"}'), \
             ('o1', 'other', '{}')",
            vec![],
        )
        .await
        .unwrap();
    handle
        .execute(
            "INSERT INTO block_tags (block_id, tag) VALUES ('q1', 'decision'), ('q2', 'decision')",
            vec![],
        )
        .await
        .unwrap();
    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW block AS SELECT b.id, b.parent_id, b.properties FROM \
             block_raw b WHERE b.id != 'sentinel:no_parent'",
        )
        .await
        .unwrap();
    let expected: BTreeSet<String> = ["q1", "q2", "q1a", "q1b", "q1a1", "q2a"]
        .into_iter()
        .map(String::from)
        .collect();
    let mut n = 0;
    for (pred_name, pred) in [("plain", PLAIN), ("exists", WITH_EXISTS)] {
        for (name, shape) in SHAPES {
            let sql = shape.replace("{P}", pred);
            let direct = handle.query(&sql, HashMap::new()).await.unwrap();
            assert_eq!(ids(&direct), expected, "direct query {pred_name}/{name}");
            n += 1;
            let view = format!("v{n}");
            handle
                .execute_ddl(&format!("CREATE MATERIALIZED VIEW {view} AS {sql}"))
                .await
                .unwrap();
            let viewed = handle
                .query(&format!("SELECT id FROM {view}"), HashMap::new())
                .await
                .unwrap();
            assert_eq!(ids(&viewed), expected, "matview {pred_name}/{name}");
        }
    }
}
