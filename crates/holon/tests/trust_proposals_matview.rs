//! Smoke test for the C5 supervision view: `TrustProposalsSchemaModule`
//! creates the `trust_proposals` matview over `block_raw`, proposal blocks
//! (rows with a `_proposal` property) project into it with their proposer
//! provenance, and `TRUST_PROPOSAL_STATS_SQL` aggregates acceptance stats per
//! origin — the "supervision = one query" payoff.

use std::collections::HashMap;

use holon::storage::schema_module::SchemaModule;
use holon::storage::turso::TursoBackend;
use holon_turso::schema_modules::CoreSchemaModule;
use holon_turso::schema_modules::TRUST_PROPOSAL_STATS_SQL;
use holon_turso::schema_modules::TrustProposalsSchemaModule;
use tempfile::TempDir;
use tokio::sync::broadcast;

fn proposal_properties(status: &str, session: &str) -> String {
    format!(
        r#"{{"_proposal":{{"status":"{status}","entity":"block","op":"create","params":{{"id":"block:x"}}}},"_provenance":{{"origin":"agent","at_millis":1,"session_id":"{session}","tool_call_id":"c1"}}}}"#
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn trust_proposals_matview_projects_and_stats_aggregate() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("trust_proposals.db");
    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, _cdc_rx) = broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");

    CoreSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("core schema");
    CoreSchemaModule
        .initialize_data(&handle)
        .await
        .expect("core seed (sentinel row)");
    TrustProposalsSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("trust_proposals matview");

    // A proposals root, two proposals from one agent session (one pending,
    // one accepted), and an ordinary block that must NOT project.
    handle
        .execute(
            "INSERT INTO block_raw (id, parent_id, content) VALUES ('block:proposals', \
             'sentinel:no_parent', 'Proposals')",
            vec![],
        )
        .await
        .expect("root insert");
    for (id, status) in [("block:p1", "pending"), ("block:p2", "accepted")] {
        handle
            .execute(
                &format!(
                    "INSERT INTO block_raw (id, parent_id, content, properties) VALUES ('{id}', \
                     'block:proposals', 'Proposal', '{}')",
                    proposal_properties(status, "sess-1")
                ),
                vec![],
            )
            .await
            .expect("proposal insert");
    }
    handle
        .execute(
            "INSERT INTO block_raw (id, parent_id, content) VALUES ('block:plain', \
             'sentinel:no_parent', 'not a proposal')",
            vec![],
        )
        .await
        .expect("plain insert");

    let rows = handle
        .query(
            "SELECT id, status, origin, session_id FROM trust_proposals ORDER BY id",
            HashMap::new(),
        )
        .await
        .expect("matview query");
    assert_eq!(rows.len(), 2, "only proposal blocks project: {rows:?}");
    let ids: Vec<String> = rows
        .iter()
        .map(|r| r.get("id").unwrap().as_string().unwrap().to_string())
        .collect();
    assert_eq!(ids, vec!["block:p1", "block:p2"]);
    for row in &rows {
        assert_eq!(row.get("origin").unwrap().as_string(), Some("agent"));
        assert_eq!(row.get("session_id").unwrap().as_string(), Some("sess-1"));
    }

    let stats = handle
        .query(TRUST_PROPOSAL_STATS_SQL, HashMap::new())
        .await
        .expect("stats query");
    assert_eq!(stats.len(), 1, "one proposer group: {stats:?}");
    let group = &stats[0];
    let int_of = |key: &str| match group.get(key) {
        Some(holon_api::Value::Integer(i)) => *i,
        other => panic!("stat '{key}' not an integer: {other:?}"),
    };
    assert_eq!(int_of("proposals"), 2);
    assert_eq!(int_of("accepted"), 1);
    assert_eq!(int_of("rejected"), 0);
    assert_eq!(int_of("pending"), 1);
}
