//! `merge_blocks` never merges away a block of a shared subtree: the merge
//! deletes what it merges away, and removing shared content is a share write
//! or, on a page shared with this device, leaving the share — neither of which
//! a merge step may do behind its own undo.

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
use holon_api::PAGE_TAG;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::link_parser::PageId;
use holon_api::share_props::SHARED_TREE_ID_PROPERTY;
use holon_core::OperationProvider;
use holon_core::storage::types::StorageEntity;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

async fn block_engine() -> Arc<BackendEngine> {
    create_test_engine_with_providers(":memory:".into(), |module| {
        module
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                let descriptors = BlockSchemaModule.edge_fields();
                Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle,
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    descriptors,
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

async fn create(engine: &BackendEngine, id: &str, parent_id: &str, extra: StorageEntity) {
    let mut params: StorageEntity = extra;
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("content".into(), Value::String("same body".to_string()));
    params.insert("parent_id".into(), Value::String(parent_id.to_string()));
    engine
        .execute_operation(&EntityName::new("block"), "create", params, OpOrigin::Sync)
        .await
        .unwrap_or_else(|e| panic!("create {id}: {e:#}"));
}

async fn exists(engine: &BackendEngine, id: &str) -> bool {
    !engine
        .db_handle()
        .query(
            &format!("SELECT id FROM {BLOCK_WRITE_TABLE} WHERE id = '{id}'"),
            HashMap::new(),
        )
        .await
        .expect("read block_raw")
        .is_empty()
}

#[tokio::test(flavor = "multi_thread")]
async fn merging_away_a_shared_block_is_refused_before_any_write() {
    let engine = block_engine().await;
    let home = PageId::for_path("Home").unwrap().as_str().to_string();
    let mut page = StorageEntity::new();
    page.insert(
        "tags".into(),
        Value::Array(vec![Value::String(PAGE_TAG.to_string())]),
    );
    create(&engine, &home, "sentinel:no_parent", page).await;
    create(&engine, "block:canonical", &home, StorageEntity::new()).await;
    let mut shared = StorageEntity::new();
    shared.insert(
        "properties".into(),
        Value::Object(HashMap::from([(
            SHARED_TREE_ID_PROPERTY.to_string(),
            Value::String("stid-x".to_string()),
        )])),
    );
    create(&engine, "block:duplicate", &home, shared).await;

    let mut params: StorageEntity = HashMap::new();
    params.insert("canonical".into(), Value::String("block:canonical".into()));
    params.insert("duplicate".into(), Value::String("block:duplicate".into()));
    let refused = engine
        .execute_operation(
            &EntityName::new("block"),
            "merge_blocks",
            params,
            OpOrigin::User,
        )
        .await
        .expect_err("a shared block is never merged away");
    assert!(
        format!("{refused:#}").contains("belongs to shared tree"),
        "the refusal names the share: {refused:#}"
    );
    assert!(
        exists(&engine, "block:duplicate").await,
        "nothing was deleted"
    );
}
