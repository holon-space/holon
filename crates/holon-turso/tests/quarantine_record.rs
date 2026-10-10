//! Contract: a quarantine and Holon's record of it are written in one
//! transaction, so a failure between the rename and the record leaves neither.

use std::collections::HashMap;

use holon_turso::turso::TursoBackend;
use tokio::sync::broadcast;

#[tokio::test(flavor = "multi_thread")]
async fn a_rename_whose_transaction_fails_is_rolled_back() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = TursoBackend::open_database(dir.path().join("q.db")).expect("open");
    let (_backend, handle) = TursoBackend::new(db, broadcast::channel(64).0).expect("backend");
    handle
        .execute_ddl("CREATE TABLE thing_raw (id TEXT PRIMARY KEY)")
        .await
        .expect("create");
    handle
        .execute("INSERT INTO thing_raw (id) VALUES ('a')", vec![])
        .await
        .expect("insert");

    let failed = handle
        .transaction(vec![
            (
                "ALTER TABLE \"thing_raw\" RENAME TO \"thing_raw__quarantined\"".to_string(),
                vec![],
            ),
            (
                "INSERT INTO no_such_record (x) VALUES (1)".to_string(),
                vec![],
            ),
        ])
        .await;
    assert!(failed.is_err(), "the record write must fail: {failed:?}");

    let tables: Vec<String> = handle
        .query(
            "SELECT name FROM sqlite_schema WHERE type = 'table' AND name LIKE 'thing%' ORDER \
             BY name",
            HashMap::new(),
        )
        .await
        .expect("list tables")
        .into_iter()
        .map(|row| format!("{:?}", row.get("name")))
        .collect();
    assert_eq!(
        tables,
        vec![format!(
            "{:?}",
            Some(holon_api::Value::String("thing_raw".into()))
        )],
        "the rename is rolled back with the failed record"
    );
    let rows = handle
        .query("SELECT id FROM thing_raw", HashMap::new())
        .await
        .expect("the table is readable under its own name");
    assert_eq!(rows.len(), 1);
}
