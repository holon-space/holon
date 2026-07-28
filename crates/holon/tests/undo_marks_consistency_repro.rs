//! Regression (BugFunnel Mac-dogfood 2026-07-19): undo of a link-bearing
//! content edit must restore the exact prior (content, marks) pair —
//! inconsistent (content, marks) pairs must NOT be representable across an
//! undo.
//!
//! Drives the REAL dispatcher + SqlOperationProvider in SqlOnly mode, captures
//! the inverse the forward `set_field("content")` returns, and replays it
//! through the SAME dispatcher entry `OperationEngine::replay` uses (so this is
//! faithful to the engine's undo path for every non-vacuous edit — the only
//! kind it journals). Then asserts the restored (content, marks) is mutually
//! consistent AND equal to the true predecessor pair.

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
use holon_api::StorageEntity;
use holon_api::Value;
use holon_core::OperationProvider;
use holon_core::UndoAction;
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

async fn setup_schema(handle: &DbHandle) {
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
    let mut p: StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String(format!("block:{id}")));
    p.insert("content".into(), Value::String(content.to_string()));
    d.execute_operation(entity, "create", p)
        .await
        .expect("create block");
}

/// Forward `set_field("content")` returning the captured inverse (the exact
/// undo entry the engine would journal).
async fn set_content_capture_inverse(
    d: &OperationDispatcher,
    entity: &EntityName,
    id: &str,
    content: &str,
) -> UndoAction {
    let mut p: StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String(format!("block:{id}")));
    p.insert("field".into(), Value::String("content".to_string()));
    p.insert("value".into(), Value::String(content.to_string()));
    d.execute_operation(entity, "set_field", p)
        .await
        .expect("set_field content")
        .undo
}

/// Replay a captured inverse through the SAME dispatcher entry `engine.replay`
/// uses (`OperationDispatcher::execute_operation`).
async fn replay_inverse(d: &OperationDispatcher, undo: UndoAction) {
    let op = match undo {
        UndoAction::Undo(op) => op,
        other => panic!("expected an invertible content edit, got {other:?}"),
    };
    let params: StorageEntity = op
        .params
        .into_iter()
        .map(|(k, v)| (Arc::from(k.as_str()), v))
        .collect();
    d.execute_operation(&op.entity_name, &op.op_name, params)
        .await
        .expect("replay inverse");
}

async fn read_content_marks(handle: &DbHandle, id: &str) -> (String, Vec<MarkSpan>) {
    let sql = format!("SELECT content, marks FROM block_raw WHERE id = 'block:{id}'");
    let rows = handle.query(&sql, HashMap::new()).await.expect("query row");
    let row = rows.into_iter().next().expect("block row present");
    let content = row
        .get("content")
        .and_then(|v| v.as_string())
        .expect("content")
        .to_string();
    let marks = match row.get("marks") {
        None | Some(Value::Null) => Vec::new(),
        Some(v) => holon_api::marks_from_json(v.as_string().expect("marks string"))
            .expect("marks JSON round-trips"),
    };
    (content, marks)
}

/// Every mark's [start,end) must lie within the content's char bounds.
fn assert_marks_in_bounds(content: &str, marks: &[MarkSpan], ctx: &str) {
    let char_len = content.chars().count();
    for (i, m) in marks.iter().enumerate() {
        assert!(
            m.start <= m.end && m.end <= char_len,
            "{ctx}: mark[{i}] {:?} range [{},{}) out of content bounds (char_len={char_len}, \
             content={content:?})",
            m.mark,
            m.start,
            m.end,
        );
    }
}

/// VECTOR A — link ADDED to plain text, then undo. Prior = ("Review PR", []).
#[tokio::test(flavor = "multi_thread")]
async fn undo_link_add_restores_prior_pair() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;
    let d = dispatcher(handle.clone());
    let entity: EntityName = ENTITY.to_string().into();

    create_block(&d, &entity, "src", "Review PR").await;
    let (prior_content, prior_marks) = read_content_marks(&handle, "src").await;
    assert_eq!(prior_content, "Review PR");
    assert!(prior_marks.is_empty());

    let undo =
        set_content_capture_inverse(&d, &entity, "src", "Review PR [[Project Falcon]]").await;
    let (c, m) = read_content_marks(&handle, "src").await;
    assert_eq!(c, "Review PR Project Falcon");
    assert_eq!(m.len(), 1);
    assert_marks_in_bounds(&c, &m, "post-edit");

    replay_inverse(&d, undo).await;
    let (c, m) = read_content_marks(&handle, "src").await;
    assert_marks_in_bounds(&c, &m, "post-undo");
    assert_eq!(
        (c.as_str(), m.as_slice()),
        (prior_content.as_str(), prior_marks.as_slice()),
        "undo must restore the EXACT prior (content, marks) pair"
    );
}

/// VECTOR B — a PRE-EXISTING link is REPLACED, then undo. Prior content carries
/// a link mark; undo must restore BOTH the prior text AND its link mark.
#[tokio::test(flavor = "multi_thread")]
async fn undo_link_replace_restores_prior_link_mark() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;
    let d = dispatcher(handle.clone());
    let entity: EntityName = ENTITY.to_string().into();

    create_block(&d, &entity, "src", "").await;
    // Establish a prior state that already has a link mark.
    set_content_capture_inverse(&d, &entity, "src", "See [[Old Page]]").await;
    let (prior_content, prior_marks) = read_content_marks(&handle, "src").await;
    assert_eq!(prior_content, "See Old Page");
    assert_eq!(prior_marks.len(), 1, "prior state has a link mark");
    assert!(matches!(prior_marks[0].mark, InlineMark::Link { .. }));

    // The edit replaces the link target.
    let undo = set_content_capture_inverse(&d, &entity, "src", "See [[Project Falcon]]").await;
    let (c, m) = read_content_marks(&handle, "src").await;
    assert_eq!(c, "See Project Falcon");
    assert_eq!(m.len(), 1);

    replay_inverse(&d, undo).await;
    let (c, m) = read_content_marks(&handle, "src").await;
    assert_marks_in_bounds(&c, &m, "post-undo");
    assert_eq!(
        (c.as_str(), m.as_slice()),
        (prior_content.as_str(), prior_marks.as_slice()),
        "undo must restore the prior link mark, not drop it"
    );
}
