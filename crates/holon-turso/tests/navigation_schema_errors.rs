//! Contract: the navigation schema surfaces every failing statement; none is
//! taken for an object that already exists.

use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::NavigationSchemaModule;
use holon_turso::turso::TursoBackend;
use tokio::sync::broadcast;

#[tokio::test(flavor = "multi_thread")]
async fn a_name_taken_by_another_object_fails_the_navigation_schema() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = TursoBackend::open_database(dir.path().join("nav.db")).expect("open");
    let (_b, handle) = TursoBackend::new(db, broadcast::channel(64).0).expect("backend");
    handle
        .execute_ddl("CREATE TABLE idx_navigation_history_region (id TEXT)")
        .await
        .expect("a table holding the index's name");

    let error = NavigationSchemaModule
        .ensure_schema(&handle)
        .await
        .expect_err("the index cannot be created, so the module must fail")
        .to_string();
    assert!(
        error.contains("idx_navigation_history_region"),
        "the error must name the object: {error}"
    );
}
