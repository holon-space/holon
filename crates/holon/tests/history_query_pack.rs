//! The canonical C2b history query pack (VisionGapAnalysis C2b, ADR 0024 P8)
//! exercised as data: each `assets/queries/history_*.sql` file is loaded via
//! `include_str!` and run against a real `block_history` relation (and, for Q4,
//! the `trust_proposals` matview), asserting the supervision/forensic answers.
//!
//! Raw SQL over `block_history` is sanctioned (Martin's ruling 2026-07-11): the
//! relation is a disclosed ephemeral cache exposed as a plain SQL table.

use std::collections::HashMap;

use holon::api::TursoHistoryStore;
use holon::storage::schema_module::SchemaModule;
use holon::storage::turso::TursoBackend;
use holon_api::HistoryEvent;
use holon_api::HistoryFidelity;
use holon_api::HistoryStore;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_turso::schema_modules::CoreSchemaModule;
use holon_turso::schema_modules::HistorySchemaModule;
use holon_turso::schema_modules::TrustProposalsSchemaModule;
use tempfile::TempDir;
use tokio::sync::broadcast;

const Q1_SUPERVISION: &str = include_str!("../../../assets/queries/history_supervision.sql");
const Q2_TRANSITIONS: &str =
    include_str!("../../../assets/queries/history_transitions_by_transition.sql");
const Q4_TRUST_FIRES: &str = include_str!("../../../assets/queries/history_trust_fires.sql");
const Q5_TIMELINE: &str = include_str!("../../../assets/queries/history_block_timeline.sql");

#[allow(clippy::too_many_arguments)]
fn ev(
    block: &str,
    op: &str,
    origin: &str,
    transition: Option<&str>,
    session: Option<&str>,
    tool_call: Option<&str>,
    field: Option<&str>,
    new_value: Option<&str>,
    at: i64,
) -> HistoryEvent {
    HistoryEvent {
        entity_name: "block".to_string(),
        block_id: block.to_string(),
        op_name: op.to_string(),
        origin: origin.to_string(),
        transition_id: transition.map(str::to_string),
        session_id: session.map(str::to_string),
        tool_call_id: tool_call.map(str::to_string),
        effect_id: None,
        field: field.map(str::to_string),
        old_value: None,
        new_value: new_value.map(str::to_string),
        at_millis: at,
        op_group: None,
    }
}

fn cell_i64(row: &StorageEntity, key: &str) -> i64 {
    match row.get(key) {
        Some(Value::Integer(i)) => *i,
        other => panic!("column '{key}' is not an integer: {other:?}"),
    }
}

fn cell_str(row: &StorageEntity, key: &str) -> Option<String> {
    match row.get(key) {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        other => panic!("column '{key}' is not TEXT/NULL: {other:?}"),
    }
}

/// A fresh in-memory backend with the `block_history` table created, plus a
/// typed store to populate it. The demo op stream: two agent-session ops on A,
/// two `rule:postpone` fires (A, B), one user op on B.
async fn seeded_history() -> (TursoBackend, holon::storage::DbHandle, TursoHistoryStore) {
    let (backend, db) = TursoBackend::new_in_memory().await.unwrap();
    HistorySchemaModule.ensure_schema(&db).await.unwrap();
    let store = TursoHistoryStore::new(db.clone(), HistoryFidelity::Loro);
    for e in [
        ev(
            "A",
            "create",
            "agent",
            None,
            Some("sess-1"),
            Some("c1"),
            None,
            None,
            10,
        ),
        ev(
            "A",
            "set_field",
            "agent",
            None,
            Some("sess-1"),
            Some("c1"),
            Some("status"),
            Some("doing"),
            20,
        ),
        ev(
            "A",
            "set_field",
            "rule",
            Some("rule:postpone"),
            None,
            None,
            Some("status"),
            Some("postponed"),
            30,
        ),
        ev(
            "B",
            "set_field",
            "rule",
            Some("rule:postpone"),
            None,
            None,
            Some("status"),
            Some("postponed"),
            40,
        ),
        ev(
            "B",
            "set_field",
            "user",
            None,
            None,
            None,
            Some("title"),
            Some("x"),
            50,
        ),
    ] {
        store.record(e).await.unwrap();
    }
    (backend, db, store)
}

#[tokio::test]
async fn q1_supervision_counts_ops_per_session_and_tool_call() {
    let (_backend, db, _store) = seeded_history().await;
    let rows = db.query(Q1_SUPERVISION, HashMap::new()).await.unwrap();
    assert_eq!(rows.len(), 2, "one row per (session, tool_call): {rows:?}");

    let agent = rows
        .iter()
        .find(|r| cell_str(r, "session_id").as_deref() == Some("sess-1"))
        .expect("the agent session row");
    assert_eq!(cell_str(agent, "tool_call_id").as_deref(), Some("c1"));
    assert_eq!(cell_i64(agent, "ops"), 2, "sess-1/c1 ran two ops");
    assert_eq!(cell_i64(agent, "events"), 2);

    let unattributed = rows
        .iter()
        .find(|r| cell_str(r, "session_id").is_none())
        .expect("the NULL-session (rule/user) row");
    assert_eq!(cell_i64(unattributed, "ops"), 3, "three non-agent ops");
}

#[tokio::test]
async fn q2_counts_ops_grouped_by_transition() {
    let (_backend, db, _store) = seeded_history().await;
    let rows = db.query(Q2_TRANSITIONS, HashMap::new()).await.unwrap();
    assert_eq!(rows.len(), 2, "NULL transition + rule:postpone: {rows:?}");

    let postpone = rows
        .iter()
        .find(|r| cell_str(r, "transition_id").as_deref() == Some("rule:postpone"))
        .expect("the rule:postpone group");
    assert_eq!(cell_i64(postpone, "ops"), 2, "rule:postpone fired twice");

    let non_rule = rows
        .iter()
        .find(|r| cell_str(r, "transition_id").is_none())
        .expect("the NULL-transition group");
    assert_eq!(cell_i64(non_rule, "ops"), 3, "three non-rule ops");
}

#[tokio::test]
async fn q5_block_timeline_is_ordered_with_provenance() {
    let (_backend, db, _store) = seeded_history().await;
    let mut params = HashMap::new();
    params.insert("block_id".to_string(), Value::String("A".to_string()));
    let rows = db.query(Q5_TIMELINE, params).await.unwrap();

    assert_eq!(rows.len(), 3, "block A has three history events: {rows:?}");
    assert_eq!(cell_str(&rows[0], "op_name").as_deref(), Some("create"));
    assert!(
        cell_i64(&rows[0], "seq") < cell_i64(&rows[2], "seq"),
        "ordered oldest→newest by seq"
    );
    // The last event is the rule postpone, carrying its firing provenance.
    assert_eq!(
        cell_str(&rows[2], "new_value").as_deref(),
        Some("postponed")
    );
    assert_eq!(cell_str(&rows[2], "origin").as_deref(), Some("rule"));
    assert_eq!(
        cell_str(&rows[2], "transition_id").as_deref(),
        Some("rule:postpone")
    );
}

fn proposal_properties(status: &str, origin: &str, transition: Option<&str>) -> String {
    let transition_field = match transition {
        Some(t) => format!(r#","transition_id":"{t}""#),
        None => String::new(),
    };
    format!(
        r#"{{"_proposal":{{"status":"{status}","entity":"block","op":"create"}},"_provenance":{{"origin":"{origin}"{transition_field},"at_millis":1}}}}"#
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn q4_joins_trust_stats_with_history_fire_counts() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("q4.db");
    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, _cdc_rx) = broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");

    CoreSchemaModule.ensure_schema(&handle).await.unwrap();
    CoreSchemaModule.initialize_data(&handle).await.unwrap();
    TrustProposalsSchemaModule
        .ensure_schema(&handle)
        .await
        .unwrap();
    HistorySchemaModule.ensure_schema(&handle).await.unwrap();

    // Proposers: a rule (rule:postpone) with two proposals, an agent with one.
    handle
        .execute(
            "INSERT INTO block_raw (id, parent_id, content) VALUES ('block:proposals', \
             'sentinel:no_parent', 'Proposals')",
            vec![],
        )
        .await
        .unwrap();
    let proposals = [
        ("block:p1", "accepted", "rule", Some("rule:postpone")),
        ("block:p2", "pending", "rule", Some("rule:postpone")),
        ("block:p3", "rejected", "agent", None),
    ];
    for (id, status, origin, transition) in proposals {
        handle
            .execute(
                &format!(
                    "INSERT INTO block_raw (id, parent_id, content, properties) VALUES ('{id}', \
                     'block:proposals', 'Proposal', '{}')",
                    proposal_properties(status, origin, transition)
                ),
                vec![],
            )
            .await
            .unwrap();
    }

    // Two rule:postpone fires actually landed; the agent proposer never fired.
    let store = TursoHistoryStore::new(handle.clone(), HistoryFidelity::Loro);
    for block in ["A", "B"] {
        store
            .record(ev(
                block,
                "set_field",
                "rule",
                Some("rule:postpone"),
                None,
                None,
                Some("status"),
                Some("postponed"),
                10,
            ))
            .await
            .unwrap();
    }

    let rows = handle.query(Q4_TRUST_FIRES, HashMap::new()).await.unwrap();
    assert_eq!(rows.len(), 2, "one row per proposer group: {rows:?}");

    let rule = rows
        .iter()
        .find(|r| cell_str(r, "transition_id").as_deref() == Some("rule:postpone"))
        .expect("the rule:postpone proposer group");
    assert_eq!(cell_str(rule, "origin").as_deref(), Some("rule"));
    assert_eq!(cell_i64(rule, "proposals"), 2);
    assert_eq!(cell_i64(rule, "accepted"), 1);
    assert_eq!(cell_i64(rule, "pending"), 1);
    assert_eq!(cell_i64(rule, "rejected"), 0);
    assert_eq!(cell_i64(rule, "fired_ops"), 2, "rule:postpone fired twice");

    let agent = rows
        .iter()
        .find(|r| cell_str(r, "origin").as_deref() == Some("agent"))
        .expect("the agent proposer group");
    assert_eq!(cell_str(agent, "transition_id"), None);
    assert_eq!(cell_i64(agent, "proposals"), 1);
    assert_eq!(cell_i64(agent, "rejected"), 1);
    assert_eq!(
        cell_i64(agent, "fired_ops"),
        0,
        "agent proposer never fired (LEFT JOIN → COALESCE 0)"
    );
}
