//! Undo INVERSE COVERAGE WAVE 1 — end-to-end proof that the CRUD ops
//! (`set_field`, `create`, `delete`, `cycle_task_state`) now carry REAL
//! inverses through the C-shaped undo machinery, on the same production
//! two-provider SqlOnly wiring `undo_move_block_e2e` uses.
//!
//! Each test asserts undo∘op ≡ identity (state read back equals pre-state) and
//! that redo is symmetric, plus the word-boundary typing coalescing that
//! `set_field` inverses enable.

use std::collections::HashMap;
use std::sync::Arc;

use holon::api::backend_engine::BackendEngine;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::EntityName;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::OpOrigin;
use holon_api::UndoOutcome;
use holon_api::Value;
use holon_api::block::Block;
use holon_core::OperationProvider;
use holon_core::storage::types::StorageEntity;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

/// Same production SqlOnly block wiring as `undo_move_block_e2e`: the CRUD
/// authority (`SqlOperationProvider`) plus the structural provider
/// (`SqlBlockOperations`).
async fn block_engine() -> Arc<BackendEngine> {
    create_test_engine_with_providers(":memory:".into(), |module| {
        module
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                // Edge-aware, exactly as prod wires the CRUD authority
                // (`event_infra_module`): a `set_field` on an edge field
                // (`requires`/`tags`) must route to the junction table and
                // build a whole-set-restore inverse, not land in `properties`.
                Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle,
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    BlockSchemaModule.edge_fields(),
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

async fn create_block(
    engine: &BackendEngine,
    id: &str,
    parent_id: &str,
    content: &str,
    origin: OpOrigin,
) {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("content".into(), Value::String(content.to_string()));
    params.insert("parent_id".into(), Value::String(parent_id.to_string()));
    engine
        .execute_operation(&EntityName::new("block"), "create", params, origin)
        .await
        .unwrap_or_else(|e| panic!("create {id}: {e:#}"));
}

async fn set_field(engine: &BackendEngine, id: &str, field: &str, value: &str, origin: OpOrigin) {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("field".into(), Value::String(field.to_string()));
    params.insert("value".into(), Value::String(value.to_string()));
    engine
        .execute_operation(&EntityName::new("block"), "set_field", params, origin)
        .await
        .unwrap_or_else(|e| panic!("set_field {id}.{field}: {e:#}"));
}

/// Read one column of a block row; `None` when the row is absent.
async fn col(engine: &BackendEngine, id: &str, column: &str) -> Option<String> {
    let rows = engine
        .db_handle()
        .query(
            &format!(
                "SELECT {column} FROM {BLOCK_WRITE_TABLE} WHERE id = '{}'",
                id.replace('\'', "''")
            ),
            HashMap::new(),
        )
        .await
        .expect("column query");
    rows.first()
        .and_then(|r| r.get(column))
        .and_then(|v| v.as_string())
        .map(str::to_string)
}

/// Read the `marks` column. `None` ONLY for SQL NULL — `marks` is a jsonb
/// column that comes back as `Value::Json`, never `Value::String`, so reading
/// it through `col` would collapse every state to `None` and pass vacuously.
async fn marks(engine: &BackendEngine, id: &str) -> Option<String> {
    let rows = engine
        .db_handle()
        .query(
            &format!(
                "SELECT marks FROM {BLOCK_WRITE_TABLE} WHERE id = '{}'",
                id.replace('\'', "''")
            ),
            HashMap::new(),
        )
        .await
        .expect("marks query");
    match rows.first().and_then(|r| r.get("marks")) {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_string().expect("marks is a JSON string").to_string()),
    }
}

/// Read a `properties` JSON entry; `None` when absent.
async fn prop(engine: &BackendEngine, id: &str, key: &str) -> Option<String> {
    let rows = engine
        .db_handle()
        .query(
            &format!(
                "SELECT json_extract(properties, '$.{key}') AS v FROM {BLOCK_WRITE_TABLE} WHERE \
                 id = '{}'",
                id.replace('\'', "''")
            ),
            HashMap::new(),
        )
        .await
        .expect("prop query");
    rows.first()
        .and_then(|r| r.get("v"))
        .and_then(|v| v.as_string())
        .map(str::to_string)
}

async fn row_exists(engine: &BackendEngine, id: &str) -> bool {
    col(engine, id, "id").await.is_some()
}

// ---------------------------------------------------------------------------
// set_field
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn set_field_content_undo_then_redo_is_identity() {
    let engine = block_engine().await;
    create_block(
        &engine,
        "block:b",
        "sentinel:no_parent",
        "before",
        OpOrigin::Sync,
    )
    .await;

    set_field(&engine, "block:b", "content", "after", OpOrigin::User).await;
    assert_eq!(
        col(&engine, "block:b", "content").await.as_deref(),
        Some("after")
    );
    assert!(engine.can_undo().await, "set_field must push an undo entry");

    assert_eq!(engine.undo().await.expect("undo"), UndoOutcome::Applied);
    assert_eq!(
        col(&engine, "block:b", "content").await.as_deref(),
        Some("before"),
        "undo must restore the pre-write content"
    );

    assert_eq!(engine.redo().await.expect("redo"), UndoOutcome::Applied);
    assert_eq!(
        col(&engine, "block:b", "content").await.as_deref(),
        Some("after"),
        "redo must re-apply the write"
    );
}

// ---------------------------------------------------------------------------
// cycle_task_state — inverse must restore BOTH task_state and its category
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn cycle_task_state_undo_restores_both_properties() {
    let engine = block_engine().await;
    create_block(
        &engine,
        "block:t",
        "sentinel:no_parent",
        "Task",
        OpOrigin::Sync,
    )
    .await;
    assert_eq!(prop(&engine, "block:t", "task_state").await, None);
    assert_eq!(prop(&engine, "block:t", "task_state_category").await, None);

    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String("block:t".to_string()));
    engine
        .execute_operation(
            &EntityName::new("block"),
            "cycle_task_state",
            params,
            OpOrigin::User,
        )
        .await
        .expect("cycle_task_state");

    let cycled = prop(&engine, "block:t", "task_state").await;
    assert_eq!(cycled.as_deref(), Some("TODO"), "first cycle → TODO");
    assert!(
        prop(&engine, "block:t", "task_state_category")
            .await
            .is_some(),
        "cycle must also write the task_state_category sidecar"
    );

    assert_eq!(engine.undo().await.expect("undo"), UndoOutcome::Applied);
    assert_eq!(
        prop(&engine, "block:t", "task_state").await,
        None,
        "undo must remove the task_state it added"
    );
    assert_eq!(
        prop(&engine, "block:t", "task_state_category").await,
        None,
        "undo must ALSO remove the category sidecar (both properties)"
    );

    assert_eq!(engine.redo().await.expect("redo"), UndoOutcome::Applied);
    assert_eq!(
        prop(&engine, "block:t", "task_state").await.as_deref(),
        Some("TODO")
    );
    assert!(
        prop(&engine, "block:t", "task_state_category")
            .await
            .is_some()
    );
}

// ---------------------------------------------------------------------------
// create ↔ delete
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn create_undo_removes_block_redo_recreates() {
    let engine = block_engine().await;
    create_block(
        &engine,
        "block:new",
        "sentinel:no_parent",
        "Fresh",
        OpOrigin::User,
    )
    .await;
    assert!(row_exists(&engine, "block:new").await);
    assert!(engine.can_undo().await, "create must push an undo entry");

    assert_eq!(engine.undo().await.expect("undo"), UndoOutcome::Applied);
    assert!(
        !row_exists(&engine, "block:new").await,
        "undo of create must delete the created block"
    );

    assert_eq!(engine.redo().await.expect("redo"), UndoOutcome::Applied);
    assert!(
        row_exists(&engine, "block:new").await,
        "redo must re-create the block under the same id"
    );
}

/// Provenance interaction: `create` now carries a REAL inverse, but a
/// rule-fired create (e.g. journal auto-create) is `OpOrigin::Rule` and must
/// NEVER enter the user's undo stack — the classification is orthogonal to
/// origin (the engine gates on origin, the provider on invertibility).
#[tokio::test(flavor = "multi_thread")]
async fn rule_fired_create_never_enters_user_stack() {
    let engine = block_engine().await;
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String("block:auto".to_string()));
    params.insert("content".into(), Value::String("Journal".to_string()));
    params.insert(
        "parent_id".into(),
        Value::String("sentinel:no_parent".to_string()),
    );
    engine
        .execute_operation(
            &EntityName::new("block"),
            "create",
            params,
            OpOrigin::Rule {
                transition_id: "rule:journal-auto-create".to_string(),
            },
        )
        .await
        .expect("rule-fired create");

    assert!(
        row_exists(&engine, "block:auto").await,
        "the row was created"
    );
    assert!(
        !engine.can_undo().await,
        "a rule-fired create must not be user-undoable"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_leaf_undo_resurrects_identical_row() {
    let engine = block_engine().await;
    create_block(
        &engine,
        "block:parent",
        "sentinel:no_parent",
        "Parent",
        OpOrigin::Sync,
    )
    .await;
    create_block(
        &engine,
        "block:leaf",
        "block:parent",
        "Leaf content",
        OpOrigin::Sync,
    )
    .await;

    let pre_sort_key = col(&engine, "block:leaf", "sort_key").await;
    let pre_content = col(&engine, "block:leaf", "content").await;
    let pre_parent = col(&engine, "block:leaf", "parent_id").await;

    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String("block:leaf".to_string()));
    engine
        .execute_operation(&EntityName::new("block"), "delete", params, OpOrigin::User)
        .await
        .expect("delete leaf");
    assert!(
        !row_exists(&engine, "block:leaf").await,
        "delete removed the leaf"
    );
    assert!(
        engine.can_undo().await,
        "leaf delete must push an undo entry"
    );

    assert_eq!(engine.undo().await.expect("undo"), UndoOutcome::Applied);
    assert!(
        row_exists(&engine, "block:leaf").await,
        "undo resurrects the leaf"
    );
    assert_eq!(
        col(&engine, "block:leaf", "sort_key").await,
        pre_sort_key,
        "resurrected row must keep its sort_key"
    );
    assert_eq!(col(&engine, "block:leaf", "content").await, pre_content);
    assert_eq!(col(&engine, "block:leaf", "parent_id").await, pre_parent);

    assert_eq!(engine.redo().await.expect("redo"), UndoOutcome::Applied);
    assert!(
        !row_exists(&engine, "block:leaf").await,
        "redo re-deletes the leaf"
    );
}

/// #22's goal, behaviourally: undo restores `marks` to a genuine SQL NULL, and
/// it does so on a row a concurrent writer has touched since the edit.
///
/// The delete arm cannot show this — its inverse fingerprints `id`, so any
/// resurrection makes undo drop as stale. The CONTENT arm can: a `set_field`
/// precondition covers only the field it wrote (`sql_operation_provider.rs`,
/// the `known_columns` guard on `changes`), and the derived `marks` follow-up's
/// result is discarded by the dispatcher, so `marks` is never fingerprinted at
/// all. A writer that moves ONLY `marks` therefore slips past `check_stale`,
/// and the rich Object inverse writes BOTH columns in one UPDATE — landing the
/// NULL restore over the interposed spans.
///
/// The concurrent writer LOSING here is current behaviour, not a ruling: the
/// stale-guard refuses on the delete arm and the inverse wins on this one. C2
/// inherits that decision (refusal vs inverse-wins); if C2 rules refusal, this
/// is the rung that flips.
#[tokio::test(flavor = "multi_thread")]
async fn undo_of_a_content_edit_restores_marks_to_null_past_a_concurrent_marks_write() {
    let engine = block_engine().await;
    create_block(
        &engine,
        "block:parent",
        "sentinel:no_parent",
        "Parent",
        OpOrigin::Sync,
    )
    .await;
    create_block(
        &engine,
        "block:leaf",
        "block:parent",
        "prior bytes",
        OpOrigin::Sync,
    )
    .await;
    assert_eq!(
        marks(&engine, "block:leaf").await,
        None,
        "the fixture must start mark-free, else the NULL restore is vacuous"
    );

    set_field(&engine, "block:leaf", "content", "replaced", OpOrigin::User).await;

    // A writer that moves ONLY `marks` — never `content`, so nothing the undo
    // entry fingerprinted has changed.
    let spans = holon_api::marks_to_json(&[MarkSpan::new(0, 5, InlineMark::Bold)]);
    engine
        .db_handle()
        .execute(
            &format!(
                "UPDATE {BLOCK_WRITE_TABLE} SET marks = '{}' WHERE id = 'block:leaf'",
                spans.replace('\'', "''")
            ),
            vec![],
        )
        .await
        .expect("interposed marks-only write");
    assert_eq!(
        marks(&engine, "block:leaf").await.as_deref(),
        Some(spans.as_str()),
        "the interposed write must land marks, else the test is vacuous"
    );

    assert_eq!(
        engine.undo().await.expect("undo"),
        UndoOutcome::Applied,
        "a marks-only write must not make the content undo stale — the \
         precondition covers `content` alone"
    );
    assert_eq!(
        col(&engine, "block:leaf", "content").await.as_deref(),
        Some("prior bytes"),
        "undo restores the prior bytes"
    );
    assert_eq!(
        marks(&engine, "block:leaf").await,
        None,
        "undo must restore `marks` to NULL — the explicit 'this block had no \
         marks' #22 exists to express"
    );
}

// ---------------------------------------------------------------------------
// edge fields (requires / tags) — inverse restores the PRIOR full SET
// (edge writes are a whole-set replace, so the inverse is a set-restore, not
// an element-wise diff). These journal an undo entry through the engine even
// though they report no column FieldDelta.
// ---------------------------------------------------------------------------

async fn set_edge(
    engine: &BackendEngine,
    id: &str,
    field: &str,
    targets: &[&str],
    origin: OpOrigin,
) {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("field".into(), Value::String(field.to_string()));
    params.insert(
        "value".into(),
        Value::Array(
            targets
                .iter()
                .map(|t| Value::String(t.to_string()))
                .collect(),
        ),
    );
    engine
        .execute_operation(&EntityName::new("block"), "set_field", params, origin)
        .await
        .unwrap_or_else(|e| panic!("set_field {id}.{field}: {e:#}"));
}

async fn read_edge(engine: &BackendEngine, table: &str, target_col: &str, id: &str) -> Vec<String> {
    let rows = engine
        .db_handle()
        .query(
            &format!(
                "SELECT {target_col} AS t FROM {table} WHERE block_id = '{}' ORDER BY {target_col}",
                id.replace('\'', "''")
            ),
            HashMap::new(),
        )
        .await
        .expect("edge query");
    rows.into_iter()
        .filter_map(|r| r.get("t").and_then(|v| v.as_string()).map(str::to_string))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn set_field_requires_undo_restores_prior_set_then_redo() {
    let engine = block_engine().await;
    for id in ["block:e", "block:d1", "block:d2"] {
        create_block(&engine, id, "sentinel:no_parent", "x", OpOrigin::Sync).await;
    }

    // Prior set (Sync — establishes state without a journal entry).
    set_edge(
        &engine,
        "block:e",
        "requires",
        &["block:d1"],
        OpOrigin::Sync,
    )
    .await;
    assert_eq!(
        read_edge(&engine, "block_requires", "required_id", "block:e").await,
        vec!["block:d1".to_string()]
    );

    // User edit: replace the whole set with {d1, d2}.
    set_edge(
        &engine,
        "block:e",
        "requires",
        &["block:d1", "block:d2"],
        OpOrigin::User,
    )
    .await;
    assert_eq!(
        read_edge(&engine, "block_requires", "required_id", "block:e").await,
        vec!["block:d1".to_string(), "block:d2".to_string()]
    );
    assert!(
        engine.can_undo().await,
        "edge set_field must push an undo entry"
    );

    // Undo restores the PRIOR full set {d1}, not an element-wise removal.
    assert_eq!(engine.undo().await.expect("undo"), UndoOutcome::Applied);
    assert_eq!(
        read_edge(&engine, "block_requires", "required_id", "block:e").await,
        vec!["block:d1".to_string()],
        "undo must restore the previous requires set"
    );

    assert_eq!(engine.redo().await.expect("redo"), UndoOutcome::Applied);
    assert_eq!(
        read_edge(&engine, "block_requires", "required_id", "block:e").await,
        vec!["block:d1".to_string(), "block:d2".to_string()],
        "redo must re-apply the edge write"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn set_field_tags_undo_restores_prior_set_then_redo() {
    let engine = block_engine().await;
    create_block(
        &engine,
        "block:tg",
        "sentinel:no_parent",
        "x",
        OpOrigin::Sync,
    )
    .await;

    set_edge(&engine, "block:tg", "tags", &["alpha"], OpOrigin::Sync).await;
    assert_eq!(
        read_edge(&engine, "block_tags", "tag", "block:tg").await,
        vec!["alpha".to_string()]
    );

    set_edge(
        &engine,
        "block:tg",
        "tags",
        &["alpha", "beta"],
        OpOrigin::User,
    )
    .await;
    assert_eq!(
        read_edge(&engine, "block_tags", "tag", "block:tg").await,
        vec!["alpha".to_string(), "beta".to_string()]
    );
    assert!(
        engine.can_undo().await,
        "tags set_field must push an undo entry"
    );

    assert_eq!(engine.undo().await.expect("undo"), UndoOutcome::Applied);
    assert_eq!(
        read_edge(&engine, "block_tags", "tag", "block:tg").await,
        vec!["alpha".to_string()],
        "undo must restore the previous tags set"
    );

    assert_eq!(engine.redo().await.expect("redo"), UndoOutcome::Applied);
    assert_eq!(
        read_edge(&engine, "block_tags", "tag", "block:tg").await,
        vec!["alpha".to_string(), "beta".to_string()],
        "redo must re-apply the tags write"
    );
}

// ---------------------------------------------------------------------------
// word-boundary coalescing — three single-char edits become ONE undo
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn typing_three_chars_coalesces_into_one_undo() {
    let engine = block_engine().await;
    create_block(
        &engine,
        "block:type",
        "sentinel:no_parent",
        "",
        OpOrigin::Sync,
    )
    .await;

    // Type "abc" one alphanumeric char at a time — each a distinct set_field,
    // all coalesced into a single word-boundary group.
    set_field(&engine, "block:type", "content", "a", OpOrigin::User).await;
    set_field(&engine, "block:type", "content", "ab", OpOrigin::User).await;
    set_field(&engine, "block:type", "content", "abc", OpOrigin::User).await;
    assert_eq!(
        col(&engine, "block:type", "content").await.as_deref(),
        Some("abc")
    );

    // A single undo restores the WHOLE group's pre-state (empty), and there is
    // no second entry underneath (the create was Sync-origin, not pushed).
    assert_eq!(engine.undo().await.expect("undo"), UndoOutcome::Applied);
    assert_eq!(
        col(&engine, "block:type", "content").await.as_deref(),
        Some(""),
        "one undo restores the pre-typing content"
    );
    assert!(
        !engine.can_undo().await,
        "the three edits coalesced into a single undo entry"
    );

    // Redo re-applies the whole coalesced run in order.
    assert_eq!(engine.redo().await.expect("redo"), UndoOutcome::Applied);
    assert_eq!(
        col(&engine, "block:type", "content").await.as_deref(),
        Some("abc")
    );
}
