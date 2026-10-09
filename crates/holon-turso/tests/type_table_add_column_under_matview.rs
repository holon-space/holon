//! Contract: `ALTER TABLE <type>_raw ADD COLUMN` works while the type's read
//! matview is persisted on disk, and the matview stays IVM-maintained after it.
//!
//! The boot-time shape check relies on this: it adds a newly declared column to
//! a stored `<type>_raw` before the matview reconcile names that column.

use std::collections::HashMap;
use std::path::Path;

use holon_api::FieldSchema;
use holon_api::TypeDefinition;
use holon_api::Value;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use holon_turso::turso_adapter::TursoAdapter;
use tokio::sync::broadcast;

fn pantry(with_note: bool) -> TypeDefinition {
    let mut fields = vec![
        FieldSchema::new("id", "TEXT").primary_key(),
        FieldSchema::new("name", "TEXT").nullable(),
    ];
    if with_note {
        fields.push(FieldSchema::new("note", "TEXT").nullable());
    }
    TypeDefinition::new("spike_pantry", fields)
}

async fn open(path: &Path) -> (TursoBackend, DbHandle) {
    let db = TursoBackend::open_database(path).expect("open");
    TursoBackend::new(db, broadcast::channel(64).0).expect("backend")
}

async fn ids_and_notes(handle: &DbHandle, sql: &str) -> Vec<(String, Option<String>)> {
    let mut rows: Vec<(String, Option<String>)> = handle
        .query(sql, HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
        .into_iter()
        .map(|r| {
            (
                r["id"].as_string().expect("id").to_string(),
                r.get("note")
                    .and_then(|v| v.as_string().map(str::to_string)),
            )
        })
        .collect();
    rows.sort();
    rows
}

#[tokio::test(flavor = "multi_thread")]
async fn add_column_under_a_persisted_matview_keeps_ivm_working() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("spike.db");

    {
        let (_b, handle) = open(&path).await;
        TursoAdapter::register(&pantry(false), &handle)
            .await
            .expect("register v1");
        for id in ["a", "b"] {
            handle
                .execute_values(
                    "INSERT INTO spike_pantry_raw (id, name) VALUES (?, ?)",
                    vec![Value::String(id.into()), Value::String(id.into())],
                )
                .await
                .expect("seed");
        }
        handle.shutdown().await.expect("shutdown");
    }

    let (_b, handle) = open(&path).await;
    let refused = handle
        .execute_ddl("ALTER TABLE spike_pantry_raw ADD COLUMN note TEXT")
        .await
        .expect_err("the engine refuses ALTER under a dependent matview");
    eprintln!("refused as expected: {refused}");
    handle
        .execute_ddl("DROP VIEW spike_pantry")
        .await
        .expect("drop the derived matview");
    handle
        .execute_ddl("ALTER TABLE spike_pantry_raw ADD COLUMN note TEXT")
        .await
        .expect("ADD COLUMN under the persisted v1 matview");

    TursoAdapter::register(&pantry(false), &handle)
        .await
        .expect("recreate the v1 matview over the altered table");
    handle
        .execute_values(
            "INSERT INTO spike_pantry_raw (id, name, note) VALUES (?, ?, ?)",
            vec![
                Value::String("c".into()),
                Value::String("c".into()),
                Value::String("n-c".into()),
            ],
        )
        .await
        .expect("write after ALTER");
    let v1_rows = handle
        .query("SELECT id FROM spike_pantry", HashMap::new())
        .await
        .expect("read v1 matview");
    assert_eq!(
        v1_rows.len(),
        3,
        "the v1 matview must see the post-ALTER write"
    );

    // Re-registering with the new declaration reconciles the matview to name
    // the new column, and IVM keeps maintaining it.
    TursoAdapter::register(&pantry(true), &handle)
        .await
        .expect("register v2 over the altered table");
    handle
        .execute_values(
            "INSERT INTO spike_pantry_raw (id, name, note) VALUES (?, ?, ?)",
            vec![
                Value::String("d".into()),
                Value::String("d".into()),
                Value::String("n-d".into()),
            ],
        )
        .await
        .expect("write after re-register");
    handle
        .execute_values(
            "UPDATE spike_pantry_raw SET note = ? WHERE id = ?",
            vec![Value::String("n-a".into()), Value::String("a".into())],
        )
        .await
        .expect("update an old row's new column");

    let expected = vec![
        ("a".to_string(), Some("n-a".to_string())),
        ("b".to_string(), None),
        ("c".to_string(), Some("n-c".to_string())),
        ("d".to_string(), Some("n-d".to_string())),
    ];
    assert_eq!(
        ids_and_notes(&handle, "SELECT id, note FROM spike_pantry").await,
        expected,
        "the v2 matview must equal the base table"
    );
    assert_eq!(
        ids_and_notes(&handle, "SELECT id, note FROM spike_pantry_raw").await,
        expected
    );
    handle.shutdown().await.expect("shutdown");

    // Across a reopen, the persisted v2 matview still follows writes.
    let (_b, handle) = open(&path).await;
    TursoAdapter::register(&pantry(true), &handle)
        .await
        .expect("register v2 after reopen");
    handle
        .execute_values(
            "INSERT INTO spike_pantry_raw (id, name, note) VALUES (?, ?, ?)",
            vec![
                Value::String("e".into()),
                Value::String("e".into()),
                Value::String("n-e".into()),
            ],
        )
        .await
        .expect("write after reopen");
    assert_eq!(
        ids_and_notes(&handle, "SELECT id, note FROM spike_pantry").await,
        ids_and_notes(&handle, "SELECT id, note FROM spike_pantry_raw").await,
        "after reopen the matview must still equal the base table"
    );
    handle.shutdown().await.expect("shutdown");
}
