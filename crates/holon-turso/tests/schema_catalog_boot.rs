//! Contract: the schema catalog reports what the ENGINE has, never what the
//! last statement said.
//!
//! Two shapes make the difference visible. A `CREATE TABLE IF NOT EXISTS`
//! against an existing table succeeds while doing nothing, and Holon has
//! relations with two competing CREATE routes of different widths (`operation`
//! is created by `sql/schema/operations.sql`, then re-issued narrower by
//! `TypeDefinition::to_create_table_sql`). And a database that already holds
//! its schema runs no CREATE at all on the next boot.

use std::sync::Arc;

use holon_turso::schema_catalog::SchemaCatalog;
use holon_turso::sql_parser::ChangeOriginInjector;
use holon_turso::sql_parser::SqlTransformer;
use holon_turso::sql_parser::apply_sql_transforms;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use tokio::sync::broadcast;

/// The production `operation` DDL: the statement that really creates the table.
const OPERATIONS_SCHEMA_DDL: &str = "CREATE TABLE IF NOT EXISTS operation (\
     id TEXT PRIMARY KEY, operation_type TEXT NOT NULL, timestamp INTEGER NOT NULL, \
     _change_origin TEXT)";

/// What `TypeDefinition::to_create_table_sql` emits for the same entity: no
/// `_change_origin`. Against an existing table it is a no-op.
const OPERATIONS_TYPEDEF_DDL: &str = "CREATE TABLE IF NOT EXISTS operation (\
     id TEXT PRIMARY KEY, operation_type TEXT NOT NULL, timestamp INTEGER NOT NULL)";

/// Run one SELECT through the real injector, then hand the rewritten SQL back
/// to the engine. A rewrite the engine cannot prepare is the failure this
/// guards.
async fn inject_and_execute(handle: &DbHandle, sql: &str) -> (String, bool) {
    let transformers: Vec<Box<dyn SqlTransformer>> =
        vec![Box::new(ChangeOriginInjector::new(handle.schema_catalog()))];
    let rewritten = apply_sql_transforms(sql, &transformers);
    let ok = handle
        .query_positional(&rewritten, vec![])
        .await
        .map_err(|e| eprintln!("rewritten SQL did not run: {rewritten} -> {e}"))
        .is_ok();
    (rewritten, ok)
}

async fn in_memory() -> DbHandle {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory backend");
    // The backend must outlive the handle for the actor to keep running.
    std::mem::forget(_backend);
    handle
}

/// R1: the wide statement created the table, a narrow no-op followed. The
/// engine still has the column, so the catalog must still say so.
#[tokio::test(flavor = "multi_thread")]
async fn a_narrow_no_op_create_cannot_unsay_a_column_the_engine_has() {
    let handle = in_memory().await;
    handle
        .execute_ddl(OPERATIONS_SCHEMA_DDL)
        .await
        .expect("the wide create is the one that makes the table");
    handle
        .execute_ddl(OPERATIONS_TYPEDEF_DDL)
        .await
        .expect("the narrow create is a no-op against the existing table");

    assert!(
        handle
            .schema_catalog()
            .declares_column("operation", "_change_origin"),
        "the engine has the column; a statement that created nothing must not unsay it"
    );

    let (rewritten, ok) = inject_and_execute(&handle, "SELECT id FROM operation").await;
    assert!(
        rewritten.contains("operation._change_origin"),
        "the column must be projected: {rewritten}"
    );
    assert!(ok, "the rewritten SQL must run against the engine");
}

/// R2, the mirror: the narrow statement created the table, a wide no-op
/// followed. Claiming the column would emit SQL the engine cannot prepare.
#[tokio::test(flavor = "multi_thread")]
async fn a_wide_no_op_create_cannot_invent_a_column_the_engine_lacks() {
    let handle = in_memory().await;
    handle
        .execute_ddl(OPERATIONS_TYPEDEF_DDL)
        .await
        .expect("the narrow create is the one that makes the table");
    handle
        .execute_ddl(OPERATIONS_SCHEMA_DDL)
        .await
        .expect("the wide create is a no-op against the existing table");

    assert!(
        !handle
            .schema_catalog()
            .declares_column("operation", "_change_origin"),
        "the engine does not have the column; a statement that created nothing must not add it"
    );

    let (rewritten, ok) = inject_and_execute(&handle, "SELECT id FROM operation").await;
    assert!(
        !rewritten.contains("_change_origin"),
        "nothing to project: {rewritten}"
    );
    assert!(ok, "the rewritten SQL must run against the engine");
}

/// A `CREATE TABLE ... AS SELECT` names no columns in its own text. The engine
/// still knows them.
#[tokio::test(flavor = "multi_thread")]
async fn a_create_table_as_select_reports_the_columns_it_really_has() {
    let handle = in_memory().await;
    handle
        .execute_ddl("CREATE TABLE src (id TEXT PRIMARY KEY, _change_origin TEXT)")
        .await
        .expect("create src");
    handle
        .execute_ddl("CREATE TABLE copy AS SELECT id, _change_origin FROM src")
        .await
        .expect("create copy");

    assert!(
        handle
            .schema_catalog()
            .declares_column("copy", "_change_origin"),
        "a CTAS relation declares whatever the engine gave it"
    );
}

/// A view over a JOIN publishes columns from both sides.
#[tokio::test(flavor = "multi_thread")]
async fn a_wildcard_view_over_a_join_reports_both_sides() {
    let handle = in_memory().await;
    handle
        .execute_ddl("CREATE TABLE a (id TEXT PRIMARY KEY)")
        .await
        .expect("create a");
    handle
        .execute_ddl("CREATE TABLE b (id TEXT PRIMARY KEY, _change_origin TEXT)")
        .await
        .expect("create b");
    handle
        .execute_ddl("CREATE VIEW joined AS SELECT * FROM a JOIN b ON a.id = b.id")
        .await
        .expect("create joined");

    assert!(
        handle
            .schema_catalog()
            .declares_column("joined", "_change_origin"),
        "the join's right-hand columns are part of the view the engine built"
    );
}

/// `ALTER TABLE ... RENAME TO` retires the old name.
///
/// The new name stays unknown until something re-derives it: this build does
/// not resolve a name the statement just created through a rename, on the same
/// connection, until a later statement. The direction is safe — an unknown
/// relation has nothing projected for it — and no production DDL renames.
#[tokio::test(flavor = "multi_thread")]
async fn a_rename_retires_the_old_name() {
    let handle = in_memory().await;
    handle
        .execute_ddl("CREATE TABLE before_rename (id TEXT PRIMARY KEY, _change_origin TEXT)")
        .await
        .expect("create");
    assert!(
        handle
            .schema_catalog()
            .declares_column("before_rename", "_change_origin"),
        "the created table is known"
    );

    handle
        .execute_ddl("ALTER TABLE before_rename RENAME TO after_rename")
        .await
        .expect("rename");

    assert!(
        !handle
            .schema_catalog()
            .declares_column("before_rename", "_change_origin"),
        "the old name no longer names anything, so nothing may be projected for it"
    );
}

/// A relation created through the QUERY path — the route the agent-facing
/// `create_table` / `drop_table` tools take — is as real as any other, and the
/// catalog must follow it in both directions.
#[tokio::test(flavor = "multi_thread")]
async fn ddl_sent_through_the_query_path_moves_the_catalog() {
    let handle = in_memory().await;
    handle
        .query_positional(
            "CREATE TABLE gizmo (id TEXT PRIMARY KEY, _change_origin TEXT)",
            vec![],
        )
        .await
        .expect("create through the query path");
    assert!(
        handle
            .schema_catalog()
            .declares_column("gizmo", "_change_origin"),
        "a table the query path really created must be in the catalog"
    );

    handle
        .query_positional("DROP TABLE gizmo", vec![])
        .await
        .expect("drop through the query path");
    assert!(
        !handle
            .schema_catalog()
            .declares_column("gizmo", "_change_origin"),
        "a table the query path really dropped must leave the catalog"
    );
}

/// A boot that creates nothing still knows the schema on disk.
#[tokio::test(flavor = "multi_thread")]
async fn a_reopened_database_declares_what_the_engine_holds() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("catalog.db");

    {
        let db = TursoBackend::open_database(&path).expect("open for seeding");
        let (_backend, handle) =
            TursoBackend::new(db, broadcast::channel(64).0).expect("backend for seeding");
        handle
            .execute_ddl(
                "CREATE TABLE widget (id TEXT PRIMARY KEY, title TEXT, _change_origin TEXT)",
            )
            .await
            .expect("create widget");
        handle
            .execute_ddl("CREATE TABLE gadget (id TEXT PRIMARY KEY, title TEXT)")
            .await
            .expect("create gadget");

        let catalog = handle.schema_catalog();
        assert!(catalog.declares_column("widget", "_change_origin"));
        assert!(!catalog.declares_column("gadget", "_change_origin"));
    }

    let db = TursoBackend::open_database(&path).expect("reopen");
    let (_backend, handle) = TursoBackend::new(db, broadcast::channel(64).0).expect("backend");
    // Force a round trip through the actor so its boot resync has run.
    handle
        .query_positional("SELECT 1", vec![])
        .await
        .expect("actor is up");

    let catalog = handle.schema_catalog();
    assert!(
        catalog.declares_column("widget", "_change_origin"),
        "a boot that creates nothing must still read the engine's schema"
    );
    assert!(
        !catalog.declares_column("gadget", "_change_origin"),
        "and must not invent a column the engine does not have"
    );
}

/// The catalog is a plain cache: nothing outside the actor writes it.
#[tokio::test(flavor = "multi_thread")]
async fn the_catalog_is_shared_by_every_handle_clone() {
    let handle = in_memory().await;
    let other: Arc<SchemaCatalog> = handle.clone().schema_catalog();
    handle
        .execute_ddl("CREATE TABLE shared (id TEXT PRIMARY KEY, _change_origin TEXT)")
        .await
        .expect("create");

    assert!(other.declares_column("shared", "_change_origin"));
}
