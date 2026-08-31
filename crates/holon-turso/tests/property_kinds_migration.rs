//! Contract: a database written before NV-1 gains the `property_kinds` column
//! on the next boot.
//!
//! `block_raw` is created with `CREATE TABLE IF NOT EXISTS`, so an existing
//! file keeps whatever shape it was written with. Without the additive ALTER,
//! the `block` matview's synthesized SELECT names a column the table does not
//! have and every read of it fails — so this is a boot-breaking omission, not
//! a fidelity nicety.

use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::CoreSchemaModule;
use holon_turso::turso::TursoBackend;
use tokio::sync::broadcast;

/// The declared `block_raw` columns as they stand on disk.
async fn columns_of_block_raw(handle: &holon_turso::turso::DbHandle) -> String {
    handle
        .query_positional(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'block_raw'",
            vec![],
        )
        .await
        .expect("block_raw must be in sqlite_master")
        .first()
        .and_then(|r| r.get("sql"))
        .and_then(|v| v.as_string().map(str::to_string))
        .expect("the stored CREATE TABLE text")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pre_nv1_block_raw_gains_property_kinds_on_boot() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("pre-nv1.db");

    // A pre-NV-1 `block_raw`: a properties bag and no kind map beside it.
    {
        let db = TursoBackend::open_database(&path).expect("open for seeding");
        let (_backend, handle) =
            TursoBackend::new(db, broadcast::channel(64).0).expect("backend for seeding");
        handle
            .execute_ddl(
                "CREATE TABLE block_raw (id TEXT PRIMARY KEY, parent_id TEXT, properties TEXT)",
            )
            .await
            .expect("create the old-shape block_raw");
        assert!(
            !columns_of_block_raw(&handle)
                .await
                .contains("property_kinds"),
            "the seeded shape must be the one that predates NV-1"
        );
    }

    let db = TursoBackend::open_database(&path).expect("reopen");
    let (_backend, handle) = TursoBackend::new(db, broadcast::channel(64).0).expect("backend");
    CoreSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("booting over a pre-NV-1 database must migrate it, not fail");

    assert!(
        columns_of_block_raw(&handle)
            .await
            .contains("property_kinds"),
        "the boot must have added the column the block matview selects"
    );

    // Idempotent: the second boot must not try to add it again.
    CoreSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("a second boot over the migrated database must be a no-op");
}
