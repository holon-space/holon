//! A mirrored row's identity column holds a scheme-prefixed string, so the
//! sidecar must declare it `TEXT`.
//!
//! Both write legs prefix it. The sync engine rewrites a fetched record's id to
//! `{scheme}:{raw}` before the row reaches storage
//! (`mcp_sync_engine::prefixed_id`, the single source of truth the full-sync
//! diff and the mirror both key on), and the vtable writeback does the same.
//! The value that lands is therefore a string whatever the source JSON held —
//! a JSON number `1` is stored as `"jp-posts:1"`.
//!
//! A sidecar that declares that column `INTEGER` is declaring a type the store
//! cannot honour. On a lone primary key that becomes `INTEGER PRIMARY KEY`, a
//! rowid alias, and Turso refuses the insert with `datatype mismatch` — every
//! sync batch of that entity fails, forever, while the connection still reports
//! itself connected. Found by the `dogfood-explorer` gate for
//! `user-connections`; see
//! `docs/Testing/bugfunnel/entries/
//! 2026-09-12-an-integer-id-column-makes-every-connection-sync-fail-with-datatype-mismatch.
//! md`.
//!
//! So the declaration is parsed, not validated: `MirrorSchema::parse` refuses a
//! non-TEXT identity column at load, where the author can still fix the file.

use holon_api::Value;
use holon_mcp_client::McpSidecar;
use holon_mcp_client::mcp_sync_engine::prefixed_id;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

/// Every SQL type a sidecar author might reach for on an id column. The point
/// of the list is that only the ones the loader ACCEPTS are carried through to
/// a real insert — a type the loader refuses can never reach SQL.
const CANDIDATE_ID_TYPES: &[&str] = &["TEXT", "INTEGER", "REAL", "BLOB"];

/// A one-entity sidecar whose id column is declared `sql_type`.
fn sidecar_yaml(sql_type: &str) -> String {
    format!(
        "schema_version: 2\n\
         display_name: \"Fixture\"\n\
         entities:\n  \
           fx_items:\n    \
             id_column: id\n    \
             schema:\n      \
               - {{ name: id,    sql_type: {sql_type}, primary_key: true }}\n      \
               - {{ name: title, sql_type: TEXT }}\n"
    )
}

async fn fresh_db() -> DbHandle {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    // Leak the backend so its actor outlives the handle for the test.
    std::mem::forget(backend);
    handle
}

#[test]
fn a_non_text_id_column_is_refused_at_load() {
    let err = McpSidecar::from_yaml(&sidecar_yaml("INTEGER")).expect_err(
        "a sidecar declaring `id` as INTEGER must be refused at load: the sync engine stores that \
         column scheme-prefixed, so the value that reaches SQL is always a string",
    );
    let msg = err.to_string();
    for expected in ["fx_items", "id", "TEXT"] {
        assert!(
            msg.contains(expected),
            "the refusal must name the entity, the column and the remedy type so the author can \
             fix the file; `{expected}` is missing from: {msg}"
        );
    }
}

#[test]
fn a_text_id_column_is_accepted() {
    McpSidecar::from_yaml(&sidecar_yaml("TEXT"))
        .expect("TEXT is the type the store actually writes, so it must load");
}

/// The value-carrying rung: for every id type the loader lets through, the id
/// the engine actually stores must INSERT into the cache table built from that
/// declaration. This is where the two halves disagreed — the mirror-key unit
/// test asserts on the key and never on the insert.
#[tokio::test]
async fn every_accepted_id_type_accepts_the_id_the_engine_stores() {
    let mut carried = Vec::new();
    let mut refused = Vec::new();

    for sql_type in CANDIDATE_ID_TYPES {
        let Ok(sidecar) = McpSidecar::from_yaml(&sidecar_yaml(sql_type)) else {
            refused.push(*sql_type);
            continue;
        };

        let table = sidecar.prefixed_name("fx_items").table_name();
        let scheme = sidecar.prefixed_name("fx_items").as_str().to_string();
        let entity = sidecar.entities.get("fx_items").expect("fixture entity");
        let td = entity
            .to_type_definition(&table, "fixture.yaml", sidecar.write_ownership("fx_items"))
            .expect("the fixture declares a schema");

        let db = fresh_db().await;
        let ddl = td.to_create_table_sql();
        db.execute_ddl(&ddl)
            .await
            .unwrap_or_else(|e| panic!("id type {sql_type}: invalid CREATE TABLE: {e}\n{ddl}"));

        // The id the engine stores for a record whose source `id` is the JSON
        // number 1 — exactly what `record_to_entity` writes into the column.
        let stored = prefixed_id(&scheme, &Value::Integer(1))
            .expect("an integer record id is prefixable by the engine");

        let insert = format!("INSERT INTO \"{table}\" (\"id\", \"title\") VALUES (?, ?)");
        db.execute_values(
            &insert,
            vec![
                Value::String(stored.clone()),
                Value::String("row".to_string()),
            ],
        )
        .await
        .unwrap_or_else(|e| {
            panic!(
                "id type {sql_type}: the loader accepted this declaration, but the id the \
                     engine stores (`{stored}`) does not go into the column it built: {e}"
            )
        });

        let rows = db
            .query(
                &format!("SELECT id FROM \"{table}\""),
                std::collections::HashMap::new(),
            )
            .await
            .unwrap_or_else(|e| panic!("id type {sql_type}: read back: {e}"));
        assert_eq!(
            rows.len(),
            1,
            "id type {sql_type}: the insert reported success but the row is not in the table"
        );

        carried.push(*sql_type);
    }

    assert!(
        !carried.is_empty(),
        "no candidate id type survived the loader, so this test proved nothing. Candidates: \
         {CANDIDATE_ID_TYPES:?}"
    );
    assert!(
        carried.contains(&"TEXT"),
        "TEXT is the type the engine actually stores and must always be carried through; \
         accepted: {carried:?}, refused: {refused:?}"
    );
}
