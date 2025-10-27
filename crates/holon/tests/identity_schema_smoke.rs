//! Smoke test for the identity schema seam.
//!
//! Verifies that `IdentitySchemaModule::ensure_schema` creates `canonical_entity`,
//! `entity_alias`, and `proposal_queue` tables with the expected shape, and that
//! a basic insert + select round-trips correctly. Operations (merge_entities,
//! propose_merge, accept_proposal, reject_proposal) land in a follow-up step;
//! this test pins the schema surface so future operation work can rely on it.

use holon::storage::schema_module::SchemaModule;
use holon::storage::turso::TursoBackend;
use holon::storage::{IdentitySchemaModule, Resource};
use std::collections::HashMap;
use tempfile::TempDir;
use tokio::sync::broadcast;

#[tokio::test]
async fn identity_schema_module_creates_three_tables() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("identity_smoke.db");
    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, _cdc_rx) = broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");

    IdentitySchemaModule
        .ensure_schema(&handle)
        .await
        .expect("ensure_schema");

    let provides = IdentitySchemaModule.provides();
    assert!(provides.contains(&Resource::schema("canonical_entity")));
    assert!(provides.contains(&Resource::schema("entity_alias")));
    assert!(provides.contains(&Resource::schema("proposal_queue")));

    for table in ["canonical_entity", "entity_alias", "proposal_queue"] {
        let rows = handle
            .query(
                &format!("SELECT name FROM sqlite_master WHERE type='table' AND name='{table}'"),
                HashMap::new(),
            )
            .await
            .expect("sqlite_master query");
        assert_eq!(
            rows.len(),
            1,
            "expected table '{table}' to exist after ensure_schema, got {} rows",
            rows.len()
        );
    }
}

#[tokio::test]
async fn identity_tables_round_trip_basic_inserts() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("identity_roundtrip.db");
    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, _cdc_rx) = broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");

    IdentitySchemaModule
        .ensure_schema(&handle)
        .await
        .expect("ensure_schema");

    handle
        .execute(
            "INSERT INTO canonical_entity (id, kind, primary_label, created_at) \
             VALUES ('canon:1', 'person', 'Sarah Chen', 1)",
            vec![],
        )
        .await
        .expect("insert canonical");

    handle
        .execute(
            "INSERT INTO entity_alias (canonical_id, system, foreign_id, confidence) \
             VALUES ('canon:1', 'todoist', 'tdo-42', 0.95)",
            vec![],
        )
        .await
        .expect("insert alias");

    handle
        .execute(
            "INSERT INTO proposal_queue (id, kind, evidence_json, status, created_at) \
             VALUES (1, 'merge', '{\"a\":\"x\",\"b\":\"y\"}', 'pending', 1)",
            vec![],
        )
        .await
        .expect("insert proposal");

    let canonical_rows = handle
        .query(
            "SELECT primary_label FROM canonical_entity WHERE id = 'canon:1'",
            HashMap::new(),
        )
        .await
        .expect("select canonical");
    assert_eq!(canonical_rows.len(), 1);

    let alias_rows = handle
        .query(
            "SELECT foreign_id FROM entity_alias WHERE canonical_id = 'canon:1'",
            HashMap::new(),
        )
        .await
        .expect("select alias");
    assert_eq!(alias_rows.len(), 1);

    let pending_rows = handle
        .query(
            "SELECT id FROM proposal_queue WHERE status = 'pending'",
            HashMap::new(),
        )
        .await
        .expect("select pending proposals");
    assert_eq!(pending_rows.len(), 1);
}

#[tokio::test]
async fn entity_alias_enforces_system_foreign_id_primary_key() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("identity_pk.db");
    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, _cdc_rx) = broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");

    IdentitySchemaModule
        .ensure_schema(&handle)
        .await
        .expect("ensure_schema");

    handle
        .execute(
            "INSERT INTO canonical_entity (id, kind, primary_label, created_at) \
             VALUES ('canon:1', 'person', 'A', 1), \
                    ('canon:2', 'person', 'B', 1)",
            vec![],
        )
        .await
        .expect("seed canonicals");

    handle
        .execute(
            "INSERT INTO entity_alias (canonical_id, system, foreign_id, confidence) \
             VALUES ('canon:1', 'todoist', 'tdo-42', 0.9)",
            vec![],
        )
        .await
        .expect("first alias");

    let dup = handle
        .execute(
            "INSERT INTO entity_alias (canonical_id, system, foreign_id, confidence) \
             VALUES ('canon:2', 'todoist', 'tdo-42', 0.9)",
            vec![],
        )
        .await;
    assert!(
        dup.is_err(),
        "duplicate (system, foreign_id) must violate PRIMARY KEY"
    );
}
