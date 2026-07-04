//! End-to-end proof that an existing real inverse (`move_block`) flows through
//! the NEW undo machinery: a real `BackendEngine` (DI-built, with
//! `enable_undo_persistence` — `undo_log` table + live `block_raw` precondition
//! reader), the production `SqlBlockOperations` provider, C-shaped entry,
//! staleness verification against the projection, and `UndoOutcome::Applied`.

use std::collections::HashMap;
use std::sync::Arc;

use holon::api::backend_engine::BackendEngine;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::UndoOutcome;
use holon_api::Value;
use holon_api::block::Block;
use holon_core::OperationProvider;
use holon_core::storage::types::StorageEntity;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

/// A real engine with the production SqlOnly block wiring: the CRUD authority
/// (`SqlOperationProvider`, create/set_field/delete) plus the structural
/// provider (`SqlBlockOperations`, which owns `move_block` and its inverse) —
/// the same two-provider split the assembly guard demands.
async fn block_engine() -> Arc<BackendEngine> {
    create_test_engine_with_providers(":memory:".into(), |module| {
        module
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                Arc::new(SqlOperationProvider::new(
                    db_handle,
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                )) as Arc<dyn OperationProvider>
            })
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                let descriptors = BlockSchemaModule.edge_fields();
                let sql_ops = Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle.clone(),
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    descriptors,
                ));
                // Point the cache at `block_raw` so provider-internal reads
                // (`get_by_id`, sibling scans) see the rows the provider writes —
                // same pattern as the `sql_block_operations` unit tests.
                let mut block_raw_type_def = Block::type_definition();
                block_raw_type_def.name = BLOCK_WRITE_TABLE.to_string();
                let cache = tokio::task::block_in_place(|| {
                    let handle = tokio::runtime::Handle::current();
                    // ALLOW(block_on): sync provider-factory closure on a multi_thread test
                    // runtime; block_in_place makes the bridge deadlock-free.
                    handle.block_on(QueryableCache::<Block>::new(db_handle, block_raw_type_def))
                })
                .expect("block_raw cache");
                Arc::new(SqlBlockOperations::new(sql_ops, Arc::new(cache)))
                    as Arc<dyn OperationProvider>
            })
    })
    .await
    .expect("test engine with block provider")
}

async fn create_block(engine: &BackendEngine, id: &str, parent_id: &str, content: &str) {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("content".into(), Value::String(content.to_string()));
    params.insert("parent_id".into(), Value::String(parent_id.to_string()));
    // Fixture setup, not the user action under test: use a non-User origin so
    // these pre-existing blocks don't land on the undo stack. (Since Wave 1,
    // `create` carries a real inverse and a User-origin create IS undoable, so
    // a User fixture here would pollute the stack the move-block assertions
    // inspect.)
    engine
        .execute_operation(&EntityName::new("block"), "create", params, OpOrigin::Sync)
        .await
        .unwrap_or_else(|e| panic!("create {id}: {e:#}"));
}

async fn parent_of(engine: &BackendEngine, id: &str) -> String {
    let rows = engine
        .db_handle()
        .query(
            &format!(
                "SELECT parent_id FROM {BLOCK_WRITE_TABLE} WHERE id = '{}'",
                id.replace('\'', "''")
            ),
            HashMap::new(),
        )
        .await
        .expect("parent_id query");
    rows.first()
        .and_then(|r| r.get("parent_id"))
        .and_then(|v| v.as_string())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("block {id} not found or parent_id NULL"))
}

#[tokio::test(flavor = "multi_thread")]
async fn move_block_then_undo_restores_state_through_new_machinery() {
    let engine = block_engine().await;

    create_block(&engine, "block:parent-a", "sentinel:no_parent", "Parent A").await;
    create_block(&engine, "block:parent-b", "sentinel:no_parent", "Parent B").await;
    create_block(&engine, "block:child", "block:parent-a", "Child").await;
    assert_eq!(parent_of(&engine, "block:child").await, "block:parent-a");

    // The user moves the child from A to B — the op with a REAL inverse.
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String("block:child".to_string()));
    params.insert(
        "parent_id".into(),
        Value::String("block:parent-b".to_string()),
    );
    engine
        .execute_operation(
            &EntityName::new("block"),
            "move_block",
            params,
            OpOrigin::User,
        )
        .await
        .expect("move_block dispatch");
    assert_eq!(parent_of(&engine, "block:child").await, "block:parent-b");
    assert!(
        engine.can_undo().await,
        "move_block must push an undo entry"
    );

    // Undo through the new machinery: precondition verified against live
    // block_raw, stored inverse replayed, entry moved to redo.
    let outcome = engine.undo().await.expect("undo call");
    assert_eq!(
        outcome,
        UndoOutcome::Applied,
        "fresh entry must verify and apply"
    );
    assert_eq!(
        parent_of(&engine, "block:child").await,
        "block:parent-a",
        "undo must restore the original parent"
    );
    assert!(engine.can_redo().await, "undone entry must be redoable");
}

#[tokio::test(flavor = "multi_thread")]
async fn rule_fired_move_never_enters_user_stack_e2e() {
    let engine = block_engine().await;
    create_block(&engine, "block:parent-a", "sentinel:no_parent", "Parent A").await;
    create_block(&engine, "block:parent-b", "sentinel:no_parent", "Parent B").await;
    create_block(&engine, "block:child", "block:parent-a", "Child").await;

    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String("block:child".to_string()));
    params.insert(
        "parent_id".into(),
        Value::String("block:parent-b".to_string()),
    );
    engine
        .execute_operation(
            &EntityName::new("block"),
            "move_block",
            params,
            OpOrigin::Rule {
                transition_id: "rule:auto-file".to_string(),
            },
        )
        .await
        .expect("rule-fired move_block");
    assert_eq!(parent_of(&engine, "block:child").await, "block:parent-b");
    assert!(
        !engine.can_undo().await,
        "a rule-fired op must not be undoable by the user"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_move_undo_drops_loudly_e2e() {
    let engine = block_engine().await;
    create_block(&engine, "block:parent-a", "sentinel:no_parent", "Parent A").await;
    create_block(&engine, "block:parent-b", "sentinel:no_parent", "Parent B").await;
    create_block(&engine, "block:parent-c", "sentinel:no_parent", "Parent C").await;
    create_block(&engine, "block:child", "block:parent-a", "Child").await;

    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String("block:child".to_string()));
    params.insert(
        "parent_id".into(),
        Value::String("block:parent-b".to_string()),
    );
    engine
        .execute_operation(
            &EntityName::new("block"),
            "move_block",
            params,
            OpOrigin::User,
        )
        .await
        .expect("move_block dispatch");

    // A non-user write (e.g. sync) moves the child AGAIN before the undo:
    // the entry's precondition (parent = B) no longer holds.
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String("block:child".to_string()));
    params.insert(
        "parent_id".into(),
        Value::String("block:parent-c".to_string()),
    );
    engine
        .execute_operation(
            &EntityName::new("block"),
            "move_block",
            params,
            OpOrigin::Sync,
        )
        .await
        .expect("concurrent sync move");
    assert_eq!(parent_of(&engine, "block:child").await, "block:parent-c");

    let outcome = engine.undo().await.expect("undo call");
    assert!(
        matches!(outcome, UndoOutcome::StaleDropped { .. }),
        "undo over a superseded move must drop loudly, got {outcome:?}"
    );
    assert_eq!(
        parent_of(&engine, "block:child").await,
        "block:parent-c",
        "a dropped entry must not mutate state"
    );
    assert!(!engine.can_undo().await, "stale entry is dropped, not kept");
}
