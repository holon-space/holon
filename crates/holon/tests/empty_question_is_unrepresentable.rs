//! A `?` question with no text is not a question: org reads `* ?` back as the
//! title `?`, so the store refuses to hold task state `?` over empty content.

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
use holon_api::Value;
use holon_api::block::Block;
use holon_core::OperationProvider;
use holon_core::storage::types::StorageEntity;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

/// The production SqlOnly block wiring: the CRUD authority plus the structural
/// provider.
async fn block_engine() -> Arc<BackendEngine> {
    create_test_engine_with_providers(":memory:".into(), |module| {
        module
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
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
                let sql_ops = Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle.clone(),
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    BlockSchemaModule.edge_fields(),
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

async fn create_block(engine: &BackendEngine, id: &str, content: &str) {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("content".into(), Value::String(content.to_string()));
    params.insert(
        "parent_id".into(),
        Value::String("sentinel:no_parent".to_string()),
    );
    engine
        .execute_operation(&EntityName::new("block"), "create", params, OpOrigin::Sync)
        .await
        .unwrap_or_else(|e| panic!("create {id}: {e:#}"));
}

async fn create_child(engine: &BackendEngine, id: &str, parent: &str) {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("content".into(), Value::String(String::new()));
    params.insert("parent_id".into(), Value::String(parent.to_string()));
    engine
        .execute_operation(&EntityName::new("block"), "create", params, OpOrigin::Sync)
        .await
        .unwrap_or_else(|e| panic!("create {id}: {e:#}"));
}

async fn set_field(engine: &BackendEngine, id: &str, field: &str, value: &str) {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("field".into(), Value::String(field.to_string()));
    params.insert("value".into(), Value::String(value.to_string()));
    engine
        .execute_operation(
            &EntityName::new("block"),
            "set_field",
            params,
            OpOrigin::Sync,
        )
        .await
        .unwrap_or_else(|e| panic!("set_field {field} on {id}: {e:#}"));
}

async fn tag_as_page(engine: &BackendEngine, id: &str) {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("tag".into(), Value::String("Page".to_string()));
    engine
        .execute_operation(&EntityName::new("block"), "add_tag", params, OpOrigin::Sync)
        .await
        .unwrap_or_else(|e| panic!("tag {id} as Page: {e:#}"));
}

async fn try_set_field(
    engine: &BackendEngine,
    id: &str,
    field: &str,
    value: &str,
) -> anyhow::Result<()> {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("field".into(), Value::String(field.to_string()));
    params.insert("value".into(), Value::String(value.to_string()));
    engine
        .execute_operation(
            &EntityName::new("block"),
            "set_field",
            params,
            OpOrigin::User,
        )
        .await
        .map(|_| ())
}

async fn cycle(engine: &BackendEngine, id: &str) {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    engine
        .execute_operation(
            &EntityName::new("block"),
            "cycle_task_state",
            params,
            OpOrigin::User,
        )
        .await
        .unwrap_or_else(|e| panic!("cycle_task_state {id}: {e:#}"));
}

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

async fn task_state(engine: &BackendEngine, id: &str) -> String {
    prop(engine, id, "task_state").await.unwrap_or_default()
}

async fn page_declaring_the_question(engine: &BackendEngine, page: &str, child: &str) {
    use holon_api::TaskState;
    let declared = serde_json::to_string(&[
        TaskState::active("TODO"),
        TaskState::active("?"),
        TaskState::done("DONE"),
    ])
    .expect("TaskState serializes");
    create_block(engine, page, "Decisions").await;
    set_field(engine, page, "todo_keywords", &declared).await;
    tag_as_page(engine, page).await;
    create_child(engine, child, page).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn setting_the_question_keyword_on_an_empty_block_is_refused() {
    let engine = block_engine().await;
    create_block(&engine, "block:empty", "").await;

    let err = try_set_field(&engine, "block:empty", "task_state", "?")
        .await
        .expect_err("an empty question must be refused");
    assert!(
        format!("{err:#}").contains("block:empty"),
        "the refusal must name the block: {err:#}"
    );
    assert_eq!(task_state(&engine, "block:empty").await, "");

    create_block(&engine, "block:asks", "pick a storage engine").await;
    try_set_field(&engine, "block:asks", "task_state", "?")
        .await
        .expect("a question with text is legal");
    assert_eq!(task_state(&engine, "block:asks").await, "?");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_cycle_skips_the_question_on_an_empty_block() {
    let engine = block_engine().await;
    page_declaring_the_question(&engine, "block:decisions", "block:empty").await;

    let mut seen = Vec::new();
    for _ in 0..3 {
        cycle(&engine, "block:empty").await;
        seen.push(task_state(&engine, "block:empty").await);
    }
    assert_eq!(seen, vec!["TODO", "DONE", ""]);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_cycle_stops_at_the_question_on_a_block_with_text() {
    let engine = block_engine().await;
    page_declaring_the_question(&engine, "block:decisions", "block:asks").await;
    set_field(&engine, "block:asks", "content", "pick a storage engine").await;

    cycle(&engine, "block:asks").await;
    cycle(&engine, "block:asks").await;
    assert_eq!(task_state(&engine, "block:asks").await, "?");
}

#[tokio::test(flavor = "multi_thread")]
async fn clearing_a_questions_text_drops_the_question() {
    let engine = block_engine().await;
    create_block(&engine, "block:asks", "pick a storage engine").await;
    set_field(&engine, "block:asks", "task_state", "?").await;

    try_set_field(&engine, "block:asks", "content", "")
        .await
        .expect("clearing the text is a legal edit");
    assert_eq!(task_state(&engine, "block:asks").await, "");

    create_block(&engine, "block:todo", "buy milk").await;
    set_field(&engine, "block:todo", "task_state", "TODO").await;
    set_field(&engine, "block:todo", "content", "").await;
    assert_eq!(
        task_state(&engine, "block:todo").await,
        "TODO",
        "only the question needs text"
    );
}
