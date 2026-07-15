//! Integration test for the `create_page_from_link` operation.
//!
//! Covers:
//! 1. Creating a multi-segment page chain (`Projects/X`) from a dangling
//!    wiki-link, both pages created at the right levels.
//! 2. Dangling link healing — the source block's `block_links` row is
//!    re-resolved to the leaf page.
//! 3. Idempotency — a second invocation creates nothing new and returns the
//!    same leaf id.
//! 4. Empty target produces an error.

use std::collections::HashMap;
use std::sync::Arc;

use holon::core::SqlOperationProvider;
use holon::storage::schema_module::EdgeFieldDescriptor;
use holon::storage::schema_module::SchemaModule;
use holon::storage::turso::TursoBackend;
use holon_api::EntityName;
use holon_api::EntityRef;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::Value;
use holon_core::OperationProvider;
use holon_turso::schema_modules::LinkSchemaModule;

const ENTITY: &str = "block";
const TABLE: &str = "block_raw";

fn tags_descriptor() -> EdgeFieldDescriptor {
    EdgeFieldDescriptor {
        entity: ENTITY.to_string(),
        field: "tags".to_string(),
        join_table: "block_tags".to_string(),
        source_col: "block_id".to_string(),
        target_col: "tag".to_string(),
    }
}

async fn setup_schema(handle: &holon::storage::turso::DbHandle) {
    handle
        .execute_ddl(
            "CREATE TABLE block_raw (
                id TEXT PRIMARY KEY,
                parent_id TEXT,
                content TEXT NOT NULL DEFAULT '',
                content_type TEXT NOT NULL DEFAULT 'text',
                properties TEXT,
                marks TEXT,
                created_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0
            )",
        )
        .await
        .expect("block_raw table");
    handle
        .execute_ddl(
            "CREATE TABLE block_tags (
                block_id TEXT NOT NULL,
                tag TEXT NOT NULL,
                PRIMARY KEY (block_id, tag),
                FOREIGN KEY (block_id) REFERENCES block_raw(id) ON DELETE CASCADE
            )",
        )
        .await
        .expect("block_tags table");
    LinkSchemaModule
        .ensure_schema(handle)
        .await
        .expect("LinkSchemaModule schema");
}

fn provider(handle: holon::storage::turso::DbHandle) -> Arc<SqlOperationProvider> {
    Arc::new(SqlOperationProvider::with_edge_fields(
        handle,
        TABLE.to_string(),
        ENTITY.to_string(),
        ENTITY.to_string(),
        vec![tags_descriptor()],
    ))
}

fn name_link_marks(name: &str, start: usize, end: usize) -> String {
    holon_api::marks_to_json(&[MarkSpan::new(
        start,
        end,
        InlineMark::Link {
            target: EntityRef::Name {
                name: name.to_string(),
            },
            label: name.to_string(),
        },
    )])
}

fn create_params(id: &str, content: &str) -> holon_api::StorageEntity {
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String(format!("block:{id}")));
    p.insert("content".into(), Value::String(content.to_string()));
    p
}

async fn link_resolved(handle: &holon::storage::turso::DbHandle, source: &str) -> Option<String> {
    let sql = format!(
        "SELECT resolved_id FROM block_links WHERE source_block_id = 'block:{source}' \
         AND kind = 'page'"
    );
    let rows = handle.query(&sql, HashMap::new()).await.expect("query");
    rows.into_iter()
        .next()
        .and_then(|r| match r.get("resolved_id") {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        })
}

async fn block_content(handle: &holon::storage::turso::DbHandle, id: &str) -> Option<String> {
    let sql = format!(
        "SELECT content FROM block_raw WHERE id = '{}'",
        id.replace('\'', "''")
    );
    handle
        .query(&sql, HashMap::new())
        .await
        .expect("query")
        .into_iter()
        .next()
        .and_then(|r| {
            r.get("content")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
        })
}

async fn block_parent(handle: &holon::storage::turso::DbHandle, id: &str) -> Option<String> {
    let sql = format!(
        "SELECT parent_id FROM block_raw WHERE id = '{}'",
        id.replace('\'', "''")
    );
    handle
        .query(&sql, HashMap::new())
        .await
        .expect("query")
        .into_iter()
        .next()
        .and_then(|r| match r.get("parent_id") {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Null) => None,
            None => None,
            _ => None,
        })
}

async fn block_has_page_tag(handle: &holon::storage::turso::DbHandle, id: &str) -> bool {
    let sql = format!(
        "SELECT 1 FROM block_tags WHERE block_id = '{}' AND tag = 'Page'",
        id.replace('\'', "''")
    );
    !handle
        .query(&sql, HashMap::new())
        .await
        .expect("query")
        .is_empty()
}

async fn block_count(handle: &holon::storage::turso::DbHandle) -> usize {
    let rows = handle
        .query("SELECT COUNT(*) as cnt FROM block_raw", HashMap::new())
        .await
        .expect("query");
    rows.first()
        .and_then(|r| r.get("cnt"))
        .and_then(|v| v.as_i64())
        .map(|n| n as usize)
        .unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread")]
async fn create_page_from_link_creates_page_chain_and_heals_dangling_link() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;
    let provider = provider(handle.clone());
    let entity: EntityName = ENTITY.to_string().into();

    // 1. Create a source block with a dangling [[Projects/X]] link.
    let mut p = create_params("src", "see [[Projects/X]] for more");
    p.insert(
        "marks".into(),
        Value::String(name_link_marks("Projects/X", 4, 15)),
    );
    provider
        .execute_operation(&entity, "create", p)
        .await
        .expect("create src");

    // Verify the link is dangling initially.
    assert!(
        link_resolved(&handle, "src").await.is_none(),
        "link should be dangling before create_page_from_link"
    );

    let initial_block_count = block_count(&handle).await;

    // 2. Invoke create_page_from_link("Projects/X").
    let mut op_params: holon_api::StorageEntity = HashMap::new();
    op_params.insert("target".into(), Value::String("Projects/X".to_string()));
    let result = provider
        .execute_operation(&entity, "create_page_from_link", op_params)
        .await
        .expect("create_page_from_link");

    // The response should contain the leaf page id.
    let leaf_id = match &result.response {
        Some(Value::String(s)) => s.clone(),
        other => panic!("Expected String response with leaf page id, got: {other:?}"),
    };
    assert!(leaf_id.starts_with("block:"), "leaf id must be a block URI");

    // 3. Assert: `Projects` page exists, Page-tagged, at the top-level parent.
    let projects_sql = "SELECT id FROM block_raw b JOIN block_tags t ON t.block_id = b.id \
                       AND t.tag = 'Page' WHERE b.content = 'Projects'";
    let proj_rows = handle
        .query(projects_sql, HashMap::new())
        .await
        .expect("query");
    assert_eq!(proj_rows.len(), 1, "exactly one Projects page must exist");
    let projects_id = proj_rows[0]
        .get("id")
        .and_then(|v| v.as_string())
        .expect("id")
        .to_string();
    // assert parent is the sentinel (top-level).
    let parent = block_parent(&handle, &projects_id).await;
    assert_eq!(
        parent.as_deref(),
        Some("sentinel:no_parent"),
        "Projects page must be top-level (sentinel:no_parent), got: {parent:?}"
    );
    assert!(
        block_has_page_tag(&handle, &projects_id).await,
        "Projects must be Page-tagged"
    );

    // 4. Assert: `X` page exists under Projects, Page-tagged.
    let x_sql = format!(
        "SELECT id FROM block_raw b JOIN block_tags t ON t.block_id = b.id AND t.tag = 'Page' \
         WHERE b.content = 'X' AND b.parent_id = '{projects_id}'"
    );
    let x_rows = handle.query(&x_sql, HashMap::new()).await.expect("query");
    assert_eq!(
        x_rows.len(),
        1,
        "exactly one X page under Projects must exist"
    );
    let x_id = x_rows[0]
        .get("id")
        .and_then(|v| v.as_string())
        .expect("id")
        .to_string();
    assert_eq!(x_id, leaf_id, "leaf id must match the returned id");
    assert!(
        block_has_page_tag(&handle, &x_id).await,
        "X must be Page-tagged"
    );
    assert_eq!(
        block_content(&handle, &x_id).await.as_deref(),
        Some("X"),
        "X block content must be 'X'"
    );

    // 5. Assert: the source block's dangling link is now healed (resolved_id = the
    //    X page).
    let resolved = link_resolved(&handle, "src").await;
    assert_eq!(
        resolved.as_deref(),
        Some(x_id.as_str()),
        "link must be healed to point at X page"
    );

    // 6. Idempotency: running again returns the same leaf id, no new blocks.
    let block_count_after_first = block_count(&handle).await;
    let mut op_params2: holon_api::StorageEntity = HashMap::new();
    op_params2.insert("target".into(), Value::String("Projects/X".to_string()));
    let result2 = provider
        .execute_operation(&entity, "create_page_from_link", op_params2)
        .await
        .expect("second create_page_from_link");

    let leaf_id2 = match &result2.response {
        Some(Value::String(s)) => s.clone(),
        other => panic!("Expected String response, got: {other:?}"),
    };
    assert_eq!(
        leaf_id2, leaf_id,
        "second invocation must return the same leaf id"
    );
    assert_eq!(
        block_count(&handle).await,
        block_count_after_first,
        "second invocation must not create new blocks"
    );
    assert_eq!(
        block_count(&handle).await,
        initial_block_count + 2,
        "only two new blocks (Projects + X) should have been created"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn create_page_from_link_empty_target_is_error() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;
    let provider = provider(handle.clone());
    let entity: EntityName = ENTITY.to_string().into();

    let mut op_params: holon_api::StorageEntity = HashMap::new();
    op_params.insert("target".into(), Value::String("".to_string()));
    let result = provider
        .execute_operation(&entity, "create_page_from_link", op_params)
        .await;
    assert!(result.is_err(), "empty target must produce an error");
}
