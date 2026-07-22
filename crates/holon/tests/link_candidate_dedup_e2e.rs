//! Regression: the `[[` link-autocomplete search must list each entity ONCE.
//!
//! `search_link_candidates` (the `[[` popup query) UNION-ALL'd a
//! "all matching blocks" branch with a "Page-tagged matching blocks" branch.
//! A Page-tagged block matches BOTH, so it appeared twice in the popup —
//! Martin's dogfooding report ("I get the same entry twice … once as a block
//! and once as a page"). This test drives the production engine query and
//! asserts no candidate id is duplicated.

use std::collections::HashMap;
use std::sync::Arc;

use holon::api::backend_engine::BackendEngine;
use holon::api::query_engine::QueryEngine;
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
use holon_core::OperationProvider;
use holon_core::storage::types::StorageEntity;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

const BLOCK: &str = "block";

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
                    BLOCK.to_string(),
                    BLOCK.to_string(),
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
                    BLOCK.to_string(),
                    BLOCK.to_string(),
                    descriptors,
                ));
                let mut block_raw_type_def = Block::type_definition();
                block_raw_type_def.name = BLOCK_WRITE_TABLE.to_string();
                let cache = tokio::task::block_in_place(|| {
                    let handle = tokio::runtime::Handle::current();
                    // ALLOW(block_on): sync provider-factory closure on a multi_thread runtime.
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

async fn create(
    engine: &BackendEngine,
    id: &str,
    parent_id: &str,
    content: &str,
    depth: i64,
    is_page: bool,
) {
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("content".into(), Value::String(content.to_string()));
    params.insert("parent_id".into(), Value::String(parent_id.to_string()));
    params.insert("depth".into(), Value::Integer(depth));
    if is_page {
        params.insert(
            "tags".into(),
            Value::Array(vec![Value::String(PAGE_TAG.to_string())]),
        );
    }
    engine
        .execute_operation(&EntityName::new(BLOCK), "create", params, OpOrigin::Sync)
        .await
        .unwrap_or_else(|e| panic!("create {id}: {e:#}"));
}

/// A `[[` search must not list a Page-tagged block twice (once as a plain
/// content match, once as a page match).
#[tokio::test(flavor = "multi_thread")]
async fn link_search_lists_each_page_once() {
    let engine = block_engine().await;

    // A page whose title matches the filter — the tempting duplicate.
    let page = "block:rust_page";
    create(&engine, page, "sentinel:no_parent", "Rust rewrite", 0, true).await;
    // A plain (non-page) content block that also matches — must still appear.
    let note = "block:rust_note";
    create(&engine, note, page, "Rust notes go here", 1, false).await;

    let candidates = engine
        .search_link_candidates("Rust")
        .await
        .expect("link search");

    let ids: Vec<String> = candidates.iter().map(|c| c.id.to_string()).collect();
    let page_hits = ids.iter().filter(|id| id.contains("rust_page")).count();
    assert_eq!(
        page_hits, 1,
        "page must appear exactly once in [[ autocomplete, got ids: {ids:?}"
    );
    assert!(
        ids.iter().any(|id| id.contains("rust_note")),
        "non-page content match must still be listed, got ids: {ids:?}"
    );
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        ids.len(),
        "no candidate id may be duplicated, got ids: {ids:?}"
    );
}
