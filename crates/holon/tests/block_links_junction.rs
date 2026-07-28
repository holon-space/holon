//! Links increment 2 — `block_links` junction + `backlinks` IVM matview,
//! driven through the PRODUCTION write path (`SqlOperationProvider`), against
//! the REAL schema (`LinkSchemaModule`, which owns both the junction and the
//! matview).
//!
//! Covered:
//! 1. create with a dangling `Name` link mark → junction row, `resolved_id`
//!    NULL, no backlinks row (lazy page creation — no placeholder minted);
//! 2. writing a Page-tagged block re-resolves dangling rows it satisfies (the
//!    dangling→resolved trigger) and the backlinks matview maintains
//!    INCREMENTALLY (row appears without any re-populate);
//! 3. a `parent/leaf` name chain resolves by leaf suffix;
//! 4. clearing marks removes the junction row and the backlinks row (IVM
//!    removal);
//! 5. deleting the target page un-resolves inbound links (soft targets:
//!    dangling is representable, no FK).

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
use holon_turso::schema_modules::block_raw_schema_sql;
use holon_turso::sql_utils::sql_statements;

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
    // Bind the PRODUCTION block_raw DDL. A hand-listed subset silently rots
    // behind the real schema and only surfaces when a matview over the missing
    // columns is queried (misleading "incompatible DBSP version" at read time).
    for stmt in sql_statements(block_raw_schema_sql()) {
        handle.execute_ddl(stmt).await.expect("block_raw schema");
    }
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
    // The REAL increment-2 schema: block_links junction + backlinks matview.
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

async fn links_rows(
    handle: &holon::storage::turso::DbHandle,
    source: &str,
) -> Vec<(String, String, Option<String>)> {
    let sql = format!(
        "SELECT target, kind, resolved_id FROM block_links WHERE source_block_id = \
         'block:{source}' ORDER BY target, kind"
    );
    let rows = handle.query(&sql, HashMap::new()).await.expect("query");
    rows.into_iter()
        .map(|r| {
            let target = r
                .get("target")
                .and_then(|v| v.as_string())
                .expect("target")
                .to_string();
            let kind = r
                .get("kind")
                .and_then(|v| v.as_string())
                .expect("kind")
                .to_string();
            let resolved = match r.get("resolved_id") {
                None | Some(Value::Null) => None,
                Some(v) => Some(v.as_string().expect("resolved_id string").to_string()),
            };
            (target, kind, resolved)
        })
        .collect()
}

async fn backlinks_rows(handle: &holon::storage::turso::DbHandle, target: &str) -> Vec<String> {
    let sql = format!("SELECT id FROM backlinks WHERE target_id = 'block:{target}' ORDER BY id");
    let rows = handle
        .query(&sql, HashMap::new())
        .await
        .expect("backlinks query");
    rows.into_iter()
        .map(|r| {
            r.get("id")
                .and_then(|v| v.as_string())
                .expect("id")
                .to_string()
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn block_links_junction_and_backlinks_matview_lifecycle() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;
    let provider = provider(handle.clone());
    let entity: EntityName = ENTITY.to_string().into();

    // 1. Source block with a dangling wiki-name link.
    let mut p = create_params("src", "see Linked Page here");
    p.insert(
        "marks".into(),
        Value::String(name_link_marks("Linked Page", 4, 15)),
    );
    provider
        .execute_operation(&entity, "create", p)
        .await
        .expect("create src");
    assert_eq!(
        links_rows(&handle, "src").await,
        vec![("Linked Page".to_string(), "page".to_string(), None)],
        "dangling name link must land as a page-kind row with NULL resolved_id"
    );

    // 2. Creating the page resolves the dangling row and the backlinks matview
    //    maintains incrementally.
    let mut p = create_params("target-page", "Linked Page");
    p.insert(
        "tags".into(),
        Value::Array(vec![Value::String("Page".to_string())]),
    );
    provider
        .execute_operation(&entity, "create", p)
        .await
        .expect("create page");
    assert_eq!(
        links_rows(&handle, "src").await,
        vec![(
            "Linked Page".to_string(),
            "page".to_string(),
            Some("block:target-page".to_string())
        )],
        "page write must re-resolve the dangling link"
    );
    assert_eq!(
        backlinks_rows(&handle, "target-page").await,
        vec!["block:src".to_string()],
        "backlinks matview must show src referencing the page (incremental)"
    );

    // 3. A parent/leaf chain resolves by leaf suffix at write time (page already
    //    exists here).
    let mut p = create_params("src2", "see Projects/Linked Page too");
    p.insert(
        "marks".into(),
        Value::String(name_link_marks("Projects/Linked Page", 4, 24)),
    );
    provider
        .execute_operation(&entity, "create", p)
        .await
        .expect("create src2");
    assert_eq!(
        links_rows(&handle, "src2").await,
        vec![(
            "Projects/Linked Page".to_string(),
            "page".to_string(),
            Some("block:target-page".to_string())
        )],
        "name-chain leaf suffix must resolve to the existing page"
    );
    assert_eq!(
        backlinks_rows(&handle, "target-page").await,
        vec!["block:src".to_string(), "block:src2".to_string()],
    );

    // 4. Clearing marks removes the junction row AND the backlinks row.
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String("block:src2".to_string()));
    p.insert("field".into(), Value::String("marks".to_string()));
    p.insert("value".into(), Value::Null);
    provider
        .execute_operation(&entity, "set_field", p)
        .await
        .expect("clear marks");
    assert_eq!(links_rows(&handle, "src2").await, vec![]);
    assert_eq!(
        backlinks_rows(&handle, "target-page").await,
        vec!["block:src".to_string()],
        "backlinks matview must drop src2 after its link is removed (IVM removal)"
    );

    // 5. Deleting the page un-resolves inbound links (soft target → dangling again)
    //    and empties its backlinks.
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String("block:target-page".to_string()));
    provider
        .execute_operation(&entity, "delete", p)
        .await
        .expect("delete page");
    assert_eq!(
        links_rows(&handle, "src").await,
        vec![("Linked Page".to_string(), "page".to_string(), None)],
        "deleting the target must un-resolve inbound links"
    );
    assert_eq!(
        backlinks_rows(&handle, "target-page").await,
        Vec::<String>::new()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn id_form_links_resolve_trivially_and_externals_are_skipped() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;
    let provider = provider(handle.clone());
    let entity: EntityName = ENTITY.to_string().into();

    let marks = holon_api::marks_to_json(&[
        MarkSpan::new(
            0,
            3,
            InlineMark::Link {
                target: EntityRef::Internal {
                    id: holon_api::EntityUri::block("other"),
                },
                label: "abc".to_string(),
            },
        ),
        MarkSpan::new(
            4,
            7,
            InlineMark::Link {
                target: EntityRef::External {
                    url: "https://example.com".to_string(),
                },
                label: "ext".to_string(),
            },
        ),
    ]);
    let mut p = create_params("src", "abc ext");
    p.insert("marks".into(), Value::String(marks));
    provider
        .execute_operation(&entity, "create", p)
        .await
        .expect("create src");
    assert_eq!(
        links_rows(&handle, "src").await,
        vec![(
            "block:other".to_string(),
            "block".to_string(),
            Some("block:other".to_string())
        )],
        "id-form link resolves trivially; external URL links are not block links"
    );
}
