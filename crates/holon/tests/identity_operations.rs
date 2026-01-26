//! Integration tests for identity operations through OperationDispatcher.
//!
//! Verifies that `merge_entities` produces a correct inverse, and that the
//! merge → undo → redo cycle round-trips state. Includes a property-style
//! test that randomizes the canonical/alias topology to surface invariants
//! (the spec calls for PBT coverage on merge + undo).

use std::collections::{HashMap, HashSet};

use holon::core::datasource::OperationProvider;
use holon::identity::{ENTITY_NAME, IdentityProvider};
use holon::storage::IdentitySchemaModule;
use holon::storage::schema_module::SchemaModule;
use holon::storage::turso::TursoBackend;
use holon_api::{Operation, Value};
use holon_core::storage::types::StorageEntity;
use proptest::prelude::*;
use tempfile::TempDir;
use tokio::sync::broadcast;

async fn make_provider() -> (TempDir, IdentityProvider) {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("identity.db");
    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, _) = broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");
    IdentitySchemaModule
        .ensure_schema(&handle)
        .await
        .expect("ensure_schema");
    (temp, IdentityProvider::new(handle))
}

async fn seed_alias(
    handle: &holon::storage::turso::DbHandle,
    canonical_id: &str,
    system: &str,
    foreign_id: &str,
    confidence: f64,
) {
    let mut params = HashMap::new();
    params.insert(
        "canonical_id".to_string(),
        Value::String(canonical_id.to_string()),
    );
    params.insert("system".to_string(), Value::String(system.to_string()));
    params.insert(
        "foreign_id".to_string(),
        Value::String(foreign_id.to_string()),
    );
    params.insert("confidence".to_string(), Value::Float(confidence));
    handle
        .query(
            "INSERT INTO entity_alias (canonical_id, system, foreign_id, confidence) \
             VALUES ($canonical_id, $system, $foreign_id, $confidence)",
            params,
        )
        .await
        .expect("seed alias");
}

async fn snapshot(
    handle: &holon::storage::turso::DbHandle,
) -> (Vec<String>, Vec<(String, String, String, f64)>) {
    let canonicals = handle
        .query(
            "SELECT id FROM canonical_entity ORDER BY id",
            HashMap::new(),
        )
        .await
        .unwrap();
    let canonical_ids: Vec<String> = canonicals
        .iter()
        .filter_map(|r| match r.get("id") {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        })
        .collect();

    let aliases = handle
        .query(
            "SELECT canonical_id, system, foreign_id, confidence \
             FROM entity_alias ORDER BY system, foreign_id",
            HashMap::new(),
        )
        .await
        .unwrap();
    let alias_rows: Vec<(String, String, String, f64)> = aliases
        .iter()
        .map(|r| {
            (
                match r.get("canonical_id") {
                    Some(Value::String(s)) => s.clone(),
                    _ => String::new(),
                },
                match r.get("system") {
                    Some(Value::String(s)) => s.clone(),
                    _ => String::new(),
                },
                match r.get("foreign_id") {
                    Some(Value::String(s)) => s.clone(),
                    _ => String::new(),
                },
                match r.get("confidence") {
                    Some(Value::Float(f)) => *f,
                    Some(Value::Integer(n)) => *n as f64,
                    _ => 0.0,
                },
            )
        })
        .collect();
    (canonical_ids, alias_rows)
}

fn op_to_params(op: &Operation) -> StorageEntity {
    op.params.clone()
}

#[tokio::test]
async fn merge_entities_rewrites_aliases_and_deletes_a() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("identity.db");
    let db = TursoBackend::open_database(&db_path).unwrap();
    let (cdc_tx, _) = broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).unwrap();
    IdentitySchemaModule.ensure_schema(&handle).await.unwrap();

    handle
        .query(
            "INSERT INTO canonical_entity (id, kind, primary_label, created_at) \
             VALUES ('a', 'person', 'Sarah A', 100), \
                    ('b', 'person', 'Sarah B', 100)",
            HashMap::new(),
        )
        .await
        .unwrap();
    seed_alias(&handle, "a", "todoist", "tdo-1", 0.95).await;
    seed_alias(&handle, "a", "jira", "jira-1", 0.80).await;
    seed_alias(&handle, "b", "github", "gh-1", 0.99).await;

    let provider = IdentityProvider::new(handle.clone());

    let mut params = StorageEntity::new();
    params.insert("canonical_a".to_string(), Value::String("a".to_string()));
    params.insert("canonical_b".to_string(), Value::String("b".to_string()));
    let result = provider
        .execute_operation(&ENTITY_NAME.into(), "merge_entities", params)
        .await
        .expect("merge");

    // Aliases under 'a' are now under 'b'.
    let (canonicals, aliases) = snapshot(&handle).await;
    assert_eq!(canonicals, vec!["b".to_string()]);
    let canonical_ids_in_aliases: HashSet<String> = aliases.iter().map(|t| t.0.clone()).collect();
    assert_eq!(
        canonical_ids_in_aliases,
        HashSet::from(["b".to_string()]),
        "all aliases should now point to canonical 'b'"
    );

    // Inverse is restore_canonical_after_merge with full snapshot.
    let inverse = match result.undo {
        holon_core::traits::UndoAction::Undo(op) => op,
        holon_core::traits::UndoAction::Irreversible => {
            panic!("merge_entities must be reversible")
        }
    };
    assert_eq!(inverse.entity_name.as_str(), ENTITY_NAME);
    assert_eq!(inverse.op_name, "restore_canonical_after_merge");
    assert_eq!(
        inverse.params.get("id"),
        Some(&Value::String("a".to_string()))
    );
    assert_eq!(
        inverse.params.get("merged_into_id"),
        Some(&Value::String("b".to_string()))
    );
}

#[tokio::test]
async fn merge_then_undo_restores_state_exactly() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("identity.db");
    let db = TursoBackend::open_database(&db_path).unwrap();
    let (cdc_tx, _) = broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).unwrap();
    IdentitySchemaModule
        .ensure_schema(&handle)
        .await
        .expect("ensure_schema");

    // Seed via raw SQL.
    handle
        .query(
            "INSERT INTO canonical_entity (id, kind, primary_label, created_at) \
             VALUES ('a', 'person', 'A', 100), \
                    ('b', 'person', 'B', 200)",
            HashMap::new(),
        )
        .await
        .unwrap();
    seed_alias(&handle, "a", "todoist", "tdo-A", 0.5).await;
    seed_alias(&handle, "a", "github", "gh-A", 0.7).await;
    seed_alias(&handle, "b", "jira", "jira-B", 0.9).await;

    let pre = snapshot(&handle).await;

    let provider = IdentityProvider::new(handle.clone());

    // Forward: merge a -> b.
    let mut params = StorageEntity::new();
    params.insert("canonical_a".to_string(), Value::String("a".to_string()));
    params.insert("canonical_b".to_string(), Value::String("b".to_string()));
    let merge_result = provider
        .execute_operation(&ENTITY_NAME.into(), "merge_entities", params.clone())
        .await
        .unwrap();

    // Undo via the inverse.
    let inverse = match merge_result.undo {
        holon_core::traits::UndoAction::Undo(op) => op,
        _ => panic!("expected reversible"),
    };
    let undo_result = provider
        .execute_operation(
            &inverse.entity_name,
            &inverse.op_name,
            op_to_params(&inverse),
        )
        .await
        .expect("undo");

    let post_undo = snapshot(&handle).await;
    assert_eq!(post_undo, pre, "undo must restore exact prior state");

    // Redo: re-execute the original forward op.
    let _ = provider
        .execute_operation(&ENTITY_NAME.into(), "merge_entities", params)
        .await
        .expect("redo");

    let post_redo = snapshot(&handle).await;
    let canonical_ids_in_aliases: HashSet<String> =
        post_redo.1.iter().map(|t| t.0.clone()).collect();
    assert_eq!(
        canonical_ids_in_aliases,
        HashSet::from(["b".to_string()]),
        "redo must rewrite all aliases to b again"
    );
    assert_eq!(post_redo.0, vec!["b".to_string()]);

    // The inverse-of-undo should be the forward merge again — confirms
    // restore's inverse is symmetric.
    let undo_inverse = match undo_result.undo {
        holon_core::traits::UndoAction::Undo(op) => op,
        _ => panic!("restore_canonical_after_merge must be reversible"),
    };
    assert_eq!(undo_inverse.op_name, "merge_entities");
    assert_eq!(
        undo_inverse.params.get("canonical_a"),
        Some(&Value::String("a".to_string()))
    );
    assert_eq!(
        undo_inverse.params.get("canonical_b"),
        Some(&Value::String("b".to_string()))
    );
}

#[tokio::test]
async fn propose_merge_then_undo_round_trips() {
    let (_temp, provider) = make_provider().await;
    let mut params = StorageEntity::new();
    params.insert("id".to_string(), Value::Integer(42));
    params.insert("kind".to_string(), Value::String("merge".to_string()));
    params.insert(
        "evidence_json".to_string(),
        Value::String(r#"{"a":"x"}"#.to_string()),
    );
    params.insert("created_at".to_string(), Value::Integer(123));

    let result = provider
        .execute_operation(&ENTITY_NAME.into(), "propose_merge", params)
        .await
        .unwrap();
    let inverse = match result.undo {
        holon_core::traits::UndoAction::Undo(op) => op,
        _ => panic!("expected reversible"),
    };
    assert_eq!(inverse.op_name, "delete_proposal");
    assert_eq!(inverse.params.get("id"), Some(&Value::Integer(42)));

    // Execute the inverse.
    provider
        .execute_operation(
            &inverse.entity_name,
            &inverse.op_name,
            op_to_params(&inverse),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn accept_proposal_undo_restores_pending_status() {
    let (_temp, provider) = make_provider().await;
    let mut params = StorageEntity::new();
    params.insert("id".to_string(), Value::Integer(7));
    params.insert("kind".to_string(), Value::String("merge".to_string()));
    params.insert("evidence_json".to_string(), Value::String("{}".to_string()));
    params.insert("created_at".to_string(), Value::Integer(1));
    provider
        .execute_operation(&ENTITY_NAME.into(), "propose_merge", params)
        .await
        .unwrap();

    let mut accept_params = StorageEntity::new();
    accept_params.insert("id".to_string(), Value::Integer(7));
    let result = provider
        .execute_operation(&ENTITY_NAME.into(), "accept_proposal", accept_params)
        .await
        .unwrap();

    let inverse = match result.undo {
        holon_core::traits::UndoAction::Undo(op) => op,
        _ => panic!("expected reversible"),
    };
    assert_eq!(inverse.op_name, "revert_proposal_status");
    assert_eq!(
        inverse.params.get("status"),
        Some(&Value::String("pending".to_string())),
        "undo must restore the previous status (pending)"
    );

    // Execute the inverse and verify status is back to pending.
    provider
        .execute_operation(
            &inverse.entity_name,
            &inverse.op_name,
            op_to_params(&inverse),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn merge_with_no_aliases_round_trips() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("identity.db");
    let db = TursoBackend::open_database(&db_path).unwrap();
    let (cdc_tx, _) = broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).unwrap();
    IdentitySchemaModule.ensure_schema(&handle).await.unwrap();

    handle
        .query(
            "INSERT INTO canonical_entity (id, kind, primary_label, created_at) \
             VALUES ('a', 'person', 'A', 1), ('b', 'person', 'B', 2)",
            HashMap::new(),
        )
        .await
        .unwrap();

    let pre = snapshot(&handle).await;
    let provider = IdentityProvider::new(handle.clone());

    let mut params = StorageEntity::new();
    params.insert("canonical_a".to_string(), Value::String("a".to_string()));
    params.insert("canonical_b".to_string(), Value::String("b".to_string()));
    let result = provider
        .execute_operation(&ENTITY_NAME.into(), "merge_entities", params)
        .await
        .unwrap();

    // Undo.
    let inverse = match result.undo {
        holon_core::traits::UndoAction::Undo(op) => op,
        _ => panic!("expected reversible"),
    };
    provider
        .execute_operation(
            &inverse.entity_name,
            &inverse.op_name,
            op_to_params(&inverse),
        )
        .await
        .unwrap();

    assert_eq!(snapshot(&handle).await, pre);
}

// -------------------------------------------------------------------------
// PBT-style coverage: random merge scenarios round-trip via undo.
// -------------------------------------------------------------------------

/// Generate a small random scenario: 2..=5 canonical entities with 0..=8 aliases
/// distributed among them. Returns (canonical_ids, aliases as (canonical_id,
/// system, foreign_id)).
fn arb_scenario() -> impl Strategy<Value = (Vec<String>, Vec<(String, String, String)>)> {
    let canonicals = prop::collection::vec("[a-d]", 2..=5).prop_map(|mut v| {
        v.sort();
        v.dedup();
        // Ensure at least 2 distinct ids.
        if v.len() < 2 {
            v.push(format!("{}_extra", v[0]));
        }
        v
    });

    canonicals.prop_flat_map(|ids| {
        let count = 0usize..=8;
        let aliases = count.prop_flat_map({
            let ids = ids.clone();
            move |n| {
                prop::collection::vec((0..ids.len(), "[0-9]{1,3}", "[a-z]{1,4}"), n).prop_map({
                    let ids = ids.clone();
                    move |triples| {
                        // Deduplicate by (system, foreign_id) to satisfy PK.
                        let mut seen: HashSet<(String, String)> = HashSet::new();
                        triples
                            .into_iter()
                            .filter_map(|(idx, fid, sys)| {
                                let key = (sys.clone(), fid.clone());
                                if seen.insert(key) {
                                    Some((ids[idx].clone(), sys, fid))
                                } else {
                                    None
                                }
                            })
                            .collect()
                    }
                })
            }
        });
        (Just(ids), aliases)
    })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 12,
        max_shrink_iters: 32,
        .. ProptestConfig::default()
    })]

    /// For any scenario, merging a random pair (a, b) and then undoing must
    /// restore the original snapshot exactly. Final state after undo == start.
    #[test]
    fn random_merge_undo_round_trips(
        (ids, aliases) in arb_scenario(),
        a_idx in 0usize..5,
        b_idx in 0usize..5,
    ) {
        // Drive async work on a runtime inside the proptest body.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let temp = TempDir::new().unwrap();
            let db_path = temp.path().join("identity.db");
            let db = TursoBackend::open_database(&db_path).unwrap();
            let (cdc_tx, _) = broadcast::channel(1024);
            let (_backend, handle) = TursoBackend::new(db, cdc_tx).unwrap();
            IdentitySchemaModule.ensure_schema(&handle).await.unwrap();

            // Seed canonicals.
            for (i, id) in ids.iter().enumerate() {
                let mut p = HashMap::new();
                p.insert("id".to_string(), Value::String(id.clone()));
                p.insert("kind".to_string(), Value::String("k".to_string()));
                p.insert(
                    "primary_label".to_string(),
                    Value::String(format!("L-{i}")),
                );
                p.insert("created_at".to_string(), Value::Integer(i as i64));
                handle
                    .query(
                        "INSERT INTO canonical_entity (id, kind, primary_label, created_at) \
                         VALUES ($id, $kind, $primary_label, $created_at)",
                        p,
                    )
                    .await
                    .unwrap();
            }

            // Seed aliases.
            for (cid, sys, fid) in &aliases {
                seed_alias(&handle, cid, sys, fid, 0.5).await;
            }

            let pre = snapshot(&handle).await;
            let a = ids[a_idx % ids.len()].clone();
            let b = ids[b_idx % ids.len()].clone();
            if a == b {
                return;
            }

            let provider = IdentityProvider::new(handle.clone());
            let mut params = StorageEntity::new();
            params.insert("canonical_a".to_string(), Value::String(a.clone()));
            params.insert("canonical_b".to_string(), Value::String(b.clone()));
            let result = provider
                .execute_operation(&ENTITY_NAME.into(), "merge_entities", params)
                .await
                .expect("merge");

            // After merge: a is gone; aliases formerly under a now under b.
            let mid = snapshot(&handle).await;
            assert!(
                !mid.0.contains(&a),
                "after merge, canonical_a must be deleted"
            );
            for (cid, _, _, _) in &mid.1 {
                assert_ne!(cid.as_str(), a.as_str(), "no alias should reference a");
            }

            // Undo.
            let inverse = match result.undo {
                holon_core::traits::UndoAction::Undo(op) => op,
                _ => panic!("expected reversible"),
            };
            provider
                .execute_operation(&inverse.entity_name, &inverse.op_name, inverse.params.clone())
                .await
                .expect("undo");

            let post = snapshot(&handle).await;
            assert_eq!(post, pre, "merge → undo must round-trip exactly");
        });
    }
}
