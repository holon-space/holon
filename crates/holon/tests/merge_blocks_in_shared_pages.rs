//! `merge_blocks` never merges away a page another device shared with this
//! one: the merge deletes what it merges away, and removing such a page is
//! leaving its share, which only a delete does. Every other block of a shared
//! page merges like any other.

use std::any::Any;
use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

use holon::api::backend_engine::BackendEngine;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::OpOrigin;
use holon_api::PAGE_TAG;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::link_parser::PageId;
use holon_api::share_props::SHARED_TREE_ID_PROPERTY;
use holon_core::OperationProvider;
use holon_core::RemovingAction;
use holon_core::ShareExitRefused;
use holon_core::cell_registry::EntityCellRegistry;
use holon_core::storage::types::StorageEntity;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

/// The pages another device shared with this one; nothing else of a cell
/// registry is reached by a merge plan.
struct ReceivedPages(Vec<EntityUri>);

#[async_trait::async_trait]
impl EntityCellRegistry for ReceivedPages {
    fn live_field_any(
        &self,
        uri: &EntityUri,
        field: &str,
        _: TypeId,
    ) -> anyhow::Result<Arc<dyn Any + Send + Sync>> {
        anyhow::bail!("a merge plan reads no cell ({uri}.{field})")
    }

    fn editable_field_any(
        &self,
        uri: &EntityUri,
        field: &str,
        _: TypeId,
    ) -> anyhow::Result<Arc<dyn Any + Send + Sync>> {
        anyhow::bail!("a merge plan edits no cell ({uri}.{field})")
    }

    fn on_entity_deleted(&self, _: &EntityUri) {}

    async fn is_share_root(&self, uri: &EntityUri) -> anyhow::Result<bool> {
        Ok(self.0.contains(uri))
    }
}

async fn block_engine() -> Arc<BackendEngine> {
    block_engine_receiving(Vec::new()).await
}

/// An engine on a device that received `pages` from another device.
async fn block_engine_receiving(pages: Vec<EntityUri>) -> Arc<BackendEngine> {
    let received = Arc::new(ReceivedPages(pages));
    create_test_engine_with_providers(":memory:".into(), move |module| {
        module
            .with_operation_provider_factory(move |backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                let descriptors = BlockSchemaModule.edge_fields();
                Arc::new(
                    SqlOperationProvider::with_edge_fields(
                        db_handle,
                        BLOCK_WRITE_TABLE.to_string(),
                        "block".to_string(),
                        "block".to_string(),
                        descriptors,
                    )
                    .with_shared_pages(received.clone()),
                ) as Arc<dyn OperationProvider>
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

async fn shared_page_with_twin_children(engine: &BackendEngine) {
    let stamp = || {
        let mut props = StorageEntity::new();
        props.insert(
            "properties".into(),
            Value::Object(HashMap::from([(
                SHARED_TREE_ID_PROPERTY.to_string(),
                Value::String("stid-x".to_string()),
            )])),
        );
        props
    };
    let home = PageId::for_path("Home").unwrap().as_str().to_string();
    let mut page = StorageEntity::new();
    page.insert(
        "tags".into(),
        Value::Array(vec![Value::String(PAGE_TAG.to_string())]),
    );
    create(engine, &home, "sentinel:no_parent", page).await;
    let mut shared_page = stamp();
    shared_page.insert(
        "tags".into(),
        Value::Array(vec![Value::String(PAGE_TAG.to_string())]),
    );
    create(engine, "block:shared-page", &home, shared_page).await;
    create(engine, "block:twin-a", "block:shared-page", stamp()).await;
    create(engine, "block:twin-b", "block:shared-page", stamp()).await;
}

async fn merge(engine: &BackendEngine, canonical: &str, duplicate: &str) -> anyhow::Result<()> {
    let mut params: StorageEntity = HashMap::new();
    params.insert("canonical".into(), Value::String(canonical.into()));
    params.insert("duplicate".into(), Value::String(duplicate.into()));
    engine
        .execute_operation(
            &EntityName::new("block"),
            "merge_blocks",
            params,
            OpOrigin::User,
        )
        .await
        .map(|_| ())
}

/// The owner's rows of a page they shared carry the share's stamp; two
/// ordinary blocks inside it merge like any others.
#[tokio::test(flavor = "multi_thread")]
async fn an_owner_merges_two_ordinary_blocks_of_a_page_they_shared() {
    let engine = block_engine().await;
    shared_page_with_twin_children(&engine).await;
    merge(&engine, "block:twin-a", "block:twin-b")
        .await
        .unwrap_or_else(|e| panic!("two members of one share merge: {e:#}"));
    assert!(
        !exists(&engine, "block:twin-b").await,
        "the duplicate is merged away"
    );
    assert!(exists(&engine, "block:twin-a").await);
}

/// A recipient merging two ordinary blocks inside a received page edits the
/// shared page; only the page itself has an exit, so only it is refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_recipient_merges_two_ordinary_blocks_inside_a_received_page() {
    let engine = block_engine_receiving(vec![EntityUri::block("shared-page")]).await;
    shared_page_with_twin_children(&engine).await;
    merge(&engine, "block:twin-a", "block:twin-b")
        .await
        .unwrap_or_else(|e| panic!("two blocks inside a received page merge: {e:#}"));
    assert!(
        !exists(&engine, "block:twin-b").await,
        "the duplicate is merged away"
    );
    assert!(exists(&engine, "block:shared-page").await);
}

/// Merging a received page away would take it off this device without
/// leaving its share, so the plan refuses it before any write.
#[tokio::test(flavor = "multi_thread")]
async fn merging_away_a_received_page_is_refused_before_any_write() {
    let page = EntityUri::block("shared-page");
    let engine = block_engine_receiving(vec![page.clone()]).await;
    shared_page_with_twin_children(&engine).await;
    let home = PageId::for_path("Home").unwrap().as_str().to_string();
    let mut twin = StorageEntity::new();
    twin.insert(
        "tags".into(),
        Value::Array(vec![Value::String(PAGE_TAG.to_string())]),
    );
    create(&engine, "block:local-page", &home, twin).await;

    let refused = merge(&engine, "block:local-page", page.as_str())
        .await
        .expect_err("a received page is never merged away");
    let refusal = ShareExitRefused {
        page: page.clone(),
        action: RemovingAction::MergeIntoAnotherBlock,
    }
    .to_string();
    assert!(
        format!("{refused:#}").contains(&refusal),
        "the refusal is the share-exit refusal: {refused:#}"
    );
    assert!(exists(&engine, page.as_str()).await, "nothing was deleted");
    assert!(
        exists(&engine, "block:twin-b").await,
        "no child was moved away"
    );
}

/// A merge whose moves would carry children from a shared tree into this
/// device's own tree is refused before any write: no move crosses trees.
#[tokio::test(flavor = "multi_thread")]
async fn a_merge_moving_children_out_of_a_shared_tree_is_refused() {
    let engine = block_engine().await;
    shared_page_with_twin_children(&engine).await;
    let home = PageId::for_path("Home").unwrap().as_str().to_string();
    create(&engine, "block:local", &home, StorageEntity::new()).await;

    let refused = merge(&engine, "block:local", "block:shared-page")
        .await
        .expect_err("children never move between trees");
    assert!(
        format!("{refused:#}").contains("live in different trees"),
        "the refusal names the two trees: {refused:#}"
    );
    assert!(
        exists(&engine, "block:shared-page").await,
        "nothing was deleted"
    );
}
