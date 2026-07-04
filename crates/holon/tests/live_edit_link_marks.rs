//! Links increment 3 — the LIVE-EDIT boundary extracts inline marks.
//!
//! A UI editor commit sends a block's content as RAW org markup
//! (`[[Page]]`, `((block))`, `*bold*`) through `set_field("content")` on the
//! `OperationDispatcher` (the UI intent boundary). Ingest already splits such
//! text into a stripped `content` label + a `marks` set at its own boundary;
//! this proves the live-edit path now does the SAME via the shared
//! `extract_inline_marks`, so:
//!   - `marks` is populated (was NULL before increment 3),
//!   - the `block_links` junction gets a row (backlinks populate),
//!   - an edit that REMOVES the link drops the junction row,
//!   - a `[[block:id][label]]` id-link resolves trivially,
//!   - the stored `content` is the rendered label (marks stripped), matching
//!     ingest, so org writeback re-emits the `[[…]]` losslessly.
//!
//! Driven through the REAL dispatcher + `SqlOperationProvider` + the REAL
//! `LinkSchemaModule` schema (junction + backlinks matview), in SqlOnly mode.

use std::collections::HashMap;
use std::sync::Arc;

use holon::api::OperationDispatcher;
use holon::core::SqlOperationProvider;
use holon::storage::schema_module::EdgeFieldDescriptor;
use holon::storage::schema_module::SchemaModule;
use holon::storage::turso::DbHandle;
use holon::storage::turso::TursoBackend;
use holon_api::EntityName;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::Value;
use holon_core::OperationProvider;
use holon_turso::schema_modules::CoreSchemaModule;
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

async fn setup_schema(handle: &DbHandle) {
    // Bind the PRODUCTION core schema module, not a hand-listed subset: it
    // silently rots behind the real schema, and re-running only its DDL drops
    // the `sentinel:no_parent` FK-anchor seed that every root block needs.
    CoreSchemaModule
        .ensure_schema(handle)
        .await
        .expect("CoreSchemaModule schema");
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

fn dispatcher(handle: DbHandle) -> OperationDispatcher {
    let provider = Arc::new(SqlOperationProvider::with_edge_fields(
        handle,
        TABLE.to_string(),
        ENTITY.to_string(),
        ENTITY.to_string(),
        vec![tags_descriptor()],
    ));
    OperationDispatcher::new(vec![provider as Arc<dyn OperationProvider>])
}

async fn create_block(d: &OperationDispatcher, entity: &EntityName, id: &str, content: &str) {
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String(format!("block:{id}")));
    p.insert("content".into(), Value::String(content.to_string()));
    d.execute_operation(entity, "create", p)
        .await
        .expect("create block");
}

async fn set_content(d: &OperationDispatcher, entity: &EntityName, id: &str, content: &str) {
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String(format!("block:{id}")));
    p.insert("field".into(), Value::String("content".to_string()));
    p.insert("value".into(), Value::String(content.to_string()));
    d.execute_operation(entity, "set_field", p)
        .await
        .expect("set_field content");
}

async fn read_content_marks(handle: &DbHandle, id: &str) -> (String, Option<String>) {
    let sql = format!("SELECT content, marks FROM block_raw WHERE id = 'block:{id}'");
    let rows = handle.query(&sql, HashMap::new()).await.expect("query row");
    let row = rows.into_iter().next().expect("block row present");
    let content = row
        .get("content")
        .and_then(|v| v.as_string())
        .expect("content")
        .to_string();
    let marks = match row.get("marks") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_string().expect("marks string").to_string()),
    };
    (content, marks)
}

async fn links_rows(handle: &DbHandle, source: &str) -> Vec<(String, String, Option<String>)> {
    let sql = format!(
        "SELECT target, kind, resolved_id FROM block_links WHERE source_block_id = \
         'block:{source}' ORDER BY target, kind"
    );
    let rows = handle
        .query(&sql, HashMap::new())
        .await
        .expect("links query");
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
                Some(v) => Some(v.as_string().expect("resolved_id").to_string()),
            };
            (target, kind, resolved)
        })
        .collect()
}

async fn backlinks_rows(handle: &DbHandle, target: &str) -> Vec<String> {
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

/// The keystone: typing `[[Wiki Name]]` in the editor now extracts marks and
/// a junction row — the exact vector that dogfood #3 caught as silently lost.
#[tokio::test(flavor = "multi_thread")]
async fn live_content_edit_extracts_page_link_marks_and_junction() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;
    let d = dispatcher(handle.clone());
    let entity: EntityName = ENTITY.to_string().into();

    // A block created empty, then the user types a wiki-name link into it.
    create_block(&d, &entity, "src", "").await;

    // BEFORE the edit: no marks, no junction (baseline).
    assert_eq!(
        read_content_marks(&handle, "src").await,
        ("".to_string(), None)
    );
    assert_eq!(links_rows(&handle, "src").await, vec![]);

    // Live edit: raw markup arrives as the content value.
    set_content(&d, &entity, "src", "[[Linked Page Test]]").await;

    // Content is stored as the STRIPPED label (matching ingest), marks present.
    let (content, marks) = read_content_marks(&handle, "src").await;
    assert_eq!(
        content, "Linked Page Test",
        "content must be the rendered label, not the raw [[...]] syntax"
    );
    let marks = marks.expect("marks must be populated after a link edit (was NULL — the bug)");
    let parsed: Vec<MarkSpan> = holon_api::marks_from_json(&marks).expect("marks JSON round-trips");
    assert_eq!(parsed.len(), 1, "one link mark");
    assert!(
        matches!(parsed[0].mark, InlineMark::Link { .. }),
        "the extracted mark is a Link"
    );

    // The junction row exists — a dangling page-kind link (lazy page create).
    assert_eq!(
        links_rows(&handle, "src").await,
        vec![("Linked Page Test".to_string(), "page".to_string(), None)],
        "a typed wiki-name link must produce a dangling page-kind junction row"
    );

    // Create the page → the dangling link resolves and backlinks populate
    // (proves the UI-authored link participates in backlinks like ingest).
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String("block:page".to_string()));
    p.insert(
        "content".into(),
        Value::String("Linked Page Test".to_string()),
    );
    p.insert(
        "tags".into(),
        Value::Array(vec![Value::String("Page".to_string())]),
    );
    d.execute_operation(&entity, "create", p)
        .await
        .expect("create page");
    assert_eq!(
        backlinks_rows(&handle, "page").await,
        vec!["block:src".to_string()],
        "backlinks matview must show the UI-authored link"
    );
}

/// An edit that REMOVES the link must drop the junction row (reconciliation
/// handles update, not just create — the DELETE-then-derive replace).
#[tokio::test(flavor = "multi_thread")]
async fn live_content_edit_removing_link_clears_junction() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;
    let d = dispatcher(handle.clone());
    let entity: EntityName = ENTITY.to_string().into();

    create_block(&d, &entity, "src", "").await;
    // The link is added by a content edit (the live-edit vector).
    set_content(&d, &entity, "src", "[[Gone Page]]").await;
    assert_eq!(
        links_rows(&handle, "src").await,
        vec![("Gone Page".to_string(), "page".to_string(), None)],
    );

    // The user deletes the link, leaving plain text.
    set_content(&d, &entity, "src", "just plain text now").await;
    let (content, marks) = read_content_marks(&handle, "src").await;
    assert_eq!(content, "just plain text now");
    assert_eq!(marks, None, "no marks after the link is removed");
    assert_eq!(
        links_rows(&handle, "src").await,
        vec![],
        "removing the link must delete the junction row (update reconciliation)"
    );
}

/// BugFunnel #66 — the blur/refocus wipe. After a link is typed and committed,
/// the SqlOnly editor hydrates its buffer from the stored (stripped) `content`
/// and re-commits THAT on blur — a `set_field("content")` carrying the label
/// with NO `[[…]]` syntax. The old follow-up nulled marks on every content
/// commit, so this second, mark-free commit replaced the live `[[link]]` with
/// plain text (marks + junction wiped). The follow-up must now recognise the
/// re-commit of the already-stored label and leave the marks untouched.
#[tokio::test(flavor = "multi_thread")]
async fn blur_recommit_of_stripped_label_preserves_marks() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;
    let d = dispatcher(handle.clone());
    let entity: EntityName = ENTITY.to_string().into();

    create_block(&d, &entity, "src", "").await;

    // The user types a wiki-name link; it commits with marks + a junction row.
    set_content(&d, &entity, "src", "[[Kept Page]]").await;
    let (content, marks) = read_content_marks(&handle, "src").await;
    assert_eq!(content, "Kept Page");
    assert!(marks.is_some(), "the initial commit must populate marks");
    assert_eq!(
        links_rows(&handle, "src").await,
        vec![("Kept Page".to_string(), "page".to_string(), None)],
    );

    // Blur/refocus re-commit: the editor sends back the STRIPPED label (exactly
    // the stored `content`, no markup) — this must be a no-op for marks.
    set_content(&d, &entity, "src", "Kept Page").await;

    let (content, marks) = read_content_marks(&handle, "src").await;
    assert_eq!(content, "Kept Page", "content unchanged by the re-commit");
    let marks = marks.expect(
        "marks must SURVIVE a blur re-commit of the stripped label (was NULLed by the \
         over-dispatching follow-up — BugFunnel #66)",
    );
    let parsed: Vec<MarkSpan> = holon_api::marks_from_json(&marks).expect("marks JSON round-trips");
    assert_eq!(parsed.len(), 1, "the single link mark is still present");
    assert!(matches!(parsed[0].mark, InlineMark::Link { .. }));
    assert_eq!(
        links_rows(&handle, "src").await,
        vec![("Kept Page".to_string(), "page".to_string(), None)],
        "the junction row must survive the blur re-commit too",
    );
}

/// A `[[block:id][label]]` id-link resolves trivially (kind=block, resolved).
#[tokio::test(flavor = "multi_thread")]
async fn live_content_edit_id_link_resolves() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;
    let d = dispatcher(handle.clone());
    let entity: EntityName = ENTITY.to_string().into();

    create_block(&d, &entity, "src", "").await;
    set_content(&d, &entity, "src", "see [[block:target-123][the target]]").await;

    let (content, marks) = read_content_marks(&handle, "src").await;
    assert_eq!(content, "see the target", "id-link label rendered inline");
    assert!(marks.is_some(), "id-link produces a mark");
    assert_eq!(
        links_rows(&handle, "src").await,
        vec![(
            "block:target-123".to_string(),
            "block".to_string(),
            Some("block:target-123".to_string())
        )],
        "an id-link resolves trivially to its target id"
    );
}
