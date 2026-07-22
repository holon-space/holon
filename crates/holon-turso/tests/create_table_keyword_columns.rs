//! `TypeDefinition::to_create_table_sql` must emit DDL that a real Turso
//! instance accepts even when sidecar schema columns are named with SQL
//! keywords (`end`, `primary`, `order`, …). The gcal integration lane hit this:
//! its `end`/`primary` columns produced `CREATE TABLE … (end TEXT, …)`, which
//! Turso rejects as a syntax error, forcing an `end_time`/`is_primary` rename
//! workaround. Quoting every identifier removes that constraint.
//!
//! This test runs against a real in-memory Turso instance (same harness the
//! sidecar_views test uses) so the assertion is "the engine accepts the DDL",
//! not "the string looks right".

use holon_api::FieldSchema;
use holon_api::TypeDefinition;
use holon_turso::turso::TursoBackend;

#[tokio::test]
async fn keyword_named_columns_produce_valid_ddl() {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend);

    // `end`, `primary`, and `order` are all SQLite reserved keywords; `end` is
    // additionally an indexed column so the CREATE INDEX path is exercised too.
    let td = TypeDefinition::new(
        "gcal_event",
        vec![
            FieldSchema::new("id", "TEXT").primary_key(),
            FieldSchema::new("end", "TEXT").indexed(),
            FieldSchema::new("primary", "INTEGER"),
            FieldSchema::new("order", "INTEGER"),
        ],
    );

    let create = td.to_create_table_sql();
    handle
        .execute_ddl(&create)
        .await
        .unwrap_or_else(|e| panic!("keyword-named columns must produce valid CREATE TABLE, got error: {e}\nDDL was:\n{create}"));

    for index in td.to_index_sql() {
        handle
            .execute_ddl(&index)
            .await
            .unwrap_or_else(|e| panic!("keyword-named indexed column must produce valid CREATE INDEX, got error: {e}\nDDL was:\n{index}"));
    }
}

#[tokio::test]
async fn keyword_named_composite_primary_key_is_valid_ddl() {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend);

    // Composite PK exercises the table-level `PRIMARY KEY (…)` clause, which
    // must also quote its column list when a member is a keyword (`end`).
    let td = TypeDefinition::new(
        "gcal_slot",
        vec![
            FieldSchema::new("end", "TEXT").primary_key(),
            FieldSchema::new("primary", "TEXT").primary_key(),
            FieldSchema::new("title", "TEXT"),
        ],
    );

    let create = td.to_create_table_sql();
    handle
        .execute_ddl(&create)
        .await
        .unwrap_or_else(|e| panic!("keyword composite PK must produce valid CREATE TABLE, got error: {e}\nDDL was:\n{create}"));
}
