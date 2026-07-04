//! End-to-end proof of the engine-level `convert_block_to_page` compound
//! (BlockToPageTransform Option B): mint a new page, move the origin's content
//! and children onto it, leave a `[[page]]` link behind, re-point inbound
//! backlinks — all as ONE reversible `UndoEntry`.
//!
//! Uses the production SqlOnly block wiring (the CRUD authority
//! `SqlOperationProvider` + the structural provider `SqlBlockOperations` that
//! owns `move_block`), so the compound dispatches its constituents exactly as
//! the UI would and undo replays their real op-level inverses.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use holon::api::backend_engine::BackendEngine;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::EntityName;
use holon_api::EntityRef;
use holon_api::EntityUri;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::OpOrigin;
use holon_api::PAGE_TAG;
use holon_api::UndoOutcome;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::link_parser::PageId;
use holon_api::marks_to_json;
use holon_core::OperationProvider;
use holon_core::storage::types::StorageEntity;
use holon_org_format::OrgRenderer;
use holon_org_format::parse_org_file;
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
    // Blocks carry a `depth` = parent.depth + 1. `move_block` RECOMPUTES it, and
    // the undo/redo staleness fingerprint compares the stored value against that
    // recompute — so a fixture with an inconsistent `depth` (e.g. a child stored
    // at 0 under a depth-1 parent) makes redo drop as stale. Production blocks
    // are projector-consistent; the test fixtures must be too.
    params.insert("depth".into(), Value::Integer(depth));
    if is_page {
        params.insert(
            "tags".into(),
            Value::Array(vec![Value::String(PAGE_TAG.to_string())]),
        );
    }
    // Fixture setup, not the action under test: non-User origin so these do not
    // pollute the undo stack the convert assertions inspect.
    engine
        .execute_operation(&EntityName::new(BLOCK), "create", params, OpOrigin::Sync)
        .await
        .unwrap_or_else(|e| panic!("create {id}: {e:#}"));
}

async fn scalar(engine: &BackendEngine, sql: &str, col: &str) -> Option<String> {
    let rows = engine
        .db_handle()
        .query(sql, HashMap::new())
        .await
        .expect("query");
    rows.first()
        .and_then(|r| r.get(col))
        .and_then(|v| v.as_string())
        .map(str::to_string)
}

async fn parent_of(engine: &BackendEngine, id: &str) -> Option<String> {
    scalar(
        engine,
        &format!("SELECT parent_id FROM {BLOCK_WRITE_TABLE} WHERE id = '{id}'"),
        "parent_id",
    )
    .await
}

async fn marks_of(engine: &BackendEngine, id: &str) -> Option<String> {
    scalar(
        engine,
        &format!("SELECT marks FROM {BLOCK_WRITE_TABLE} WHERE id = '{id}'"),
        "marks",
    )
    .await
}

async fn resolved_id_of(engine: &BackendEngine, source: &str) -> Option<String> {
    scalar(
        engine,
        &format!("SELECT resolved_id FROM block_links WHERE source_block_id = '{source}'"),
        "resolved_id",
    )
    .await
}

async fn is_page(engine: &BackendEngine, id: &str) -> bool {
    !engine
        .db_handle()
        .query(
            &format!("SELECT 1 FROM block_tags WHERE block_id = '{id}' AND tag = '{PAGE_TAG}'"),
            HashMap::new(),
        )
        .await
        .expect("tag query")
        .is_empty()
}

async fn convert(engine: &BackendEngine, origin: &str, destination_path: &str) -> String {
    let mut params: StorageEntity = HashMap::new();
    params.insert("target".into(), Value::String(origin.to_string()));
    params.insert(
        "destination_path".into(),
        Value::String(destination_path.to_string()),
    );
    let out = engine
        .execute_operation(
            &EntityName::new(BLOCK),
            "convert_block_to_page",
            params,
            OpOrigin::User,
        )
        .await
        .expect("convert_block_to_page dispatch")
        .response
        .expect("convert returns the new page id");
    match out {
        Value::String(s) => s,
        other => panic!("expected page id string, got {other:?}"),
    }
}

/// A block linking to `target` — its marks carry a full-span internal link, so
/// the create derives a `block_links` row resolved to `target`.
async fn create_linker(
    engine: &BackendEngine,
    id: &str,
    parent_id: &str,
    depth: i64,
    target: &str,
) {
    let label = "see".to_string();
    let span = MarkSpan::new(
        0,
        label.chars().count(),
        InlineMark::Link {
            target: EntityRef::from_uri(&holon_api::EntityUri::from_raw(target)),
            label: label.clone(),
        },
    );
    let mut params: StorageEntity = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("content".into(), Value::String(label));
    params.insert("parent_id".into(), Value::String(parent_id.to_string()));
    params.insert("depth".into(), Value::Integer(depth));
    params.insert("marks".into(), Value::String(marks_to_json(&[span])));
    engine
        .execute_operation(&EntityName::new(BLOCK), "create", params, OpOrigin::Sync)
        .await
        .unwrap_or_else(|e| panic!("create linker {id}: {e:#}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn convert_then_undo_round_trip_restores_everything() {
    let engine = block_engine().await;

    let home = PageId::for_path("Home").unwrap().as_str().to_string();
    create(&engine, &home, "sentinel:no_parent", "Home", 0, true).await;
    let origin = "block:origin";
    create(&engine, origin, &home, "Rust rewrite", 1, false).await;
    create(&engine, "block:c1", origin, "child one", 2, false).await;
    create(&engine, "block:c2", origin, "child two", 2, false).await;
    // An inbound reference to the origin — its resolved_id must follow to P.
    create_linker(&engine, "block:linker", &home, 1, origin).await;
    assert_eq!(
        resolved_id_of(&engine, "block:linker").await.as_deref(),
        Some(origin),
        "precondition: linker resolves to origin"
    );
    let origin_marks_before = marks_of(&engine, origin).await;

    let page = convert(&engine, origin, "Home").await;

    // The new page P: deterministic id, Page tag, content moved, under Home.
    assert_eq!(
        page,
        PageId::for_path("Home/Rust rewrite").unwrap().as_str()
    );
    assert!(is_page(&engine, &page).await, "P must be a page");
    assert_eq!(
        parent_of(&engine, &page).await.as_deref(),
        Some(home.as_str())
    );
    assert_eq!(
        scalar(
            &engine,
            &format!("SELECT content FROM {BLOCK_WRITE_TABLE} WHERE id = '{page}'"),
            "content"
        )
        .await
        .as_deref(),
        Some("Rust rewrite")
    );
    // Children re-homed under P, order preserved.
    assert_eq!(
        parent_of(&engine, "block:c1").await.as_deref(),
        Some(page.as_str())
    );
    assert_eq!(
        parent_of(&engine, "block:c2").await.as_deref(),
        Some(page.as_str())
    );
    // Origin stays a non-page, keeps its text, now links to P.
    assert!(!is_page(&engine, origin).await, "origin stays a non-page");
    assert_eq!(
        resolved_id_of(&engine, origin).await.as_deref(),
        Some(page.as_str()),
        "origin's own link resolves to P"
    );
    // Inbound backlink followed origin → P.
    assert_eq!(
        resolved_id_of(&engine, "block:linker").await.as_deref(),
        Some(page.as_str()),
        "inbound backlink re-pointed to P"
    );
    assert!(
        engine.can_undo().await,
        "the compound must push ONE undo entry"
    );

    // ---- UNDO: everything comes back ----
    let outcome = engine.undo().await.expect("undo call");
    assert_eq!(outcome, UndoOutcome::Applied, "composite undo applies");

    assert!(
        scalar(
            &engine,
            &format!("SELECT id FROM {BLOCK_WRITE_TABLE} WHERE id = '{page}'"),
            "id"
        )
        .await
        .is_none(),
        "undo deletes the new page P"
    );
    assert_eq!(
        parent_of(&engine, "block:c1").await.as_deref(),
        Some(origin),
        "undo re-homes children back to origin"
    );
    assert_eq!(
        parent_of(&engine, "block:c2").await.as_deref(),
        Some(origin)
    );
    assert_eq!(
        marks_of(&engine, origin).await,
        origin_marks_before,
        "undo restores the origin's original marks (link removed)"
    );
    assert_eq!(
        resolved_id_of(&engine, "block:linker").await.as_deref(),
        Some(origin),
        "undo restores the inbound backlink to origin"
    );
    assert!(engine.can_redo().await, "the undone compound is redoable");

    // ---- REDO: re-runs the forward constituents ----
    let redo = engine.redo().await.expect("redo call");
    assert_eq!(redo, UndoOutcome::Applied, "redo re-applies");
    assert_eq!(
        parent_of(&engine, "block:c1").await.as_deref(),
        Some(page.as_str()),
        "redo re-homes children under P again"
    );
    assert!(is_page(&engine, &page).await, "redo recreates P");
}

#[tokio::test(flavor = "multi_thread")]
async fn convert_creates_missing_destination_hierarchy_reversibly() {
    let engine = block_engine().await;

    let home = PageId::for_path("Home").unwrap().as_str().to_string();
    create(&engine, &home, "sentinel:no_parent", "Home", 0, true).await;
    let origin = "block:origin2";
    create(&engine, origin, &home, "Widget", 1, false).await;

    // "Home/NewSub" — NewSub does not exist yet: the transform mints it.
    let new_sub = PageId::for_path("Home/NewSub")
        .unwrap()
        .as_str()
        .to_string();
    let page = convert(&engine, origin, "Home/NewSub").await;

    assert!(
        is_page(&engine, &new_sub).await,
        "missing hierarchy page created"
    );
    assert_eq!(
        parent_of(&engine, &new_sub).await.as_deref(),
        Some(home.as_str())
    );
    assert_eq!(
        page,
        PageId::for_path("Home/NewSub/Widget").unwrap().as_str()
    );
    assert_eq!(
        parent_of(&engine, &page).await.as_deref(),
        Some(new_sub.as_str())
    );

    engine.undo().await.expect("undo").assert_applied();
    assert!(
        scalar(
            &engine,
            &format!("SELECT id FROM {BLOCK_WRITE_TABLE} WHERE id = '{new_sub}'"),
            "id"
        )
        .await
        .is_none(),
        "undo deletes the freshly-minted hierarchy page too"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn convert_at_vault_root_is_allowed() {
    let engine = block_engine().await;
    let origin = "block:root-origin";
    create(
        &engine,
        origin,
        "sentinel:no_parent",
        "Loose note",
        0,
        false,
    )
    .await;

    let page = convert(&engine, origin, "").await;
    assert_eq!(page, PageId::for_path("Loose note").unwrap().as_str());
    assert!(is_page(&engine, &page).await);
    assert_eq!(
        parent_of(&engine, &page).await.as_deref(),
        Some("sentinel:no_parent"),
        "empty destination path lands the page at the vault root"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn convert_without_destination_defaults_to_nearest_page_ancestor() {
    let engine = block_engine().await;
    // Home(page) > Section(page) > origin(non-page). Omitting destination_path
    // must pre-select the NEAREST page ancestor (Section), not Home or root.
    let home = PageId::for_path("Home").unwrap().as_str().to_string();
    let section = PageId::for_path("Home/Section")
        .unwrap()
        .as_str()
        .to_string();
    create(&engine, &home, "sentinel:no_parent", "Home", 0, true).await;
    create(&engine, &section, &home, "Section", 1, true).await;
    let origin = "block:nested-origin";
    create(&engine, origin, &section, "Deep note", 2, false).await;

    // No destination_path param at all.
    let mut params: StorageEntity = HashMap::new();
    params.insert("target".into(), Value::String(origin.to_string()));
    let page = match engine
        .execute_operation(
            &EntityName::new(BLOCK),
            "convert_block_to_page",
            params,
            OpOrigin::User,
        )
        .await
        .expect("convert dispatch")
        .response
        .expect("page id")
    {
        Value::String(s) => s,
        other => panic!("expected id, got {other:?}"),
    };

    assert_eq!(
        page,
        PageId::for_path("Home/Section/Deep note").unwrap().as_str()
    );
    assert_eq!(
        parent_of(&engine, &page).await.as_deref(),
        Some(section.as_str()),
        "new page lands under the nearest page ancestor"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn convert_rejects_an_already_page_origin() {
    let engine = block_engine().await;
    let home = PageId::for_path("Home").unwrap().as_str().to_string();
    create(&engine, &home, "sentinel:no_parent", "Home", 0, true).await;
    let already = PageId::for_path("Home/Existing")
        .unwrap()
        .as_str()
        .to_string();
    create(&engine, &already, &home, "Existing", 1, true).await;

    let mut params: StorageEntity = HashMap::new();
    params.insert("target".into(), Value::String(already.clone()));
    params.insert("destination_path".into(), Value::String("Home".into()));
    let err = engine
        .execute_operation(
            &EntityName::new(BLOCK),
            "convert_block_to_page",
            params,
            OpOrigin::User,
        )
        .await
        .expect_err("converting a page to a page must fail loud");
    assert!(
        err.to_string().contains("already a page"),
        "fail-loud message names the cause, got: {err:#}"
    );
}

/// Bug (C) regression (GPUI dogfood 2026-07-20): the origin block's CONTENT is
/// a page TITLE, not a `/`-separated path. A `/` in the content — the
/// menu-trigger `/` still trailing when convert dispatches, or a legitimate
/// title like `"buy milk/eggs"` — must NOT be split into page-path segments.
/// Before the fix the planner did `format!("{destination}/{leaf}")` then
/// `PageId::for_path`, so a TRAILING `/` produced an "empty segment" fail-loud
/// (the exact dogfood symptom — convert looked like a silent no-op) and an
/// EMBEDDED `/` silently minted a multi-segment page id (phantom hierarchy).
#[tokio::test(flavor = "multi_thread")]
async fn convert_treats_slash_in_content_as_title_not_path() {
    // Case 1 — trailing-slash content at the vault root = the exact dogfood
    // repro (menu trigger `/` left in the origin's content at plan time).
    let engine = block_engine().await;
    let origin = "block:slashy-origin";
    create(
        &engine,
        origin,
        "sentinel:no_parent",
        "Promote me to page/",
        0,
        false,
    )
    .await;

    let page = convert(&engine, origin, "").await;
    assert_eq!(
        page,
        PageId::for_page_under("", "Promote me to page")
            .unwrap()
            .as_str(),
        "content is a single-segment title; a trailing '/' is stripped at the \
         convert boundary so the title, its id, and its filename all agree \
         (and it never splits into an empty path segment)"
    );
    assert!(is_page(&engine, &page).await);

    // Case 2 — embedded-slash content under a page ancestor stays ONE leaf.
    let home = PageId::for_path("Home").unwrap().as_str().to_string();
    create(&engine, &home, "sentinel:no_parent", "Home", 0, true).await;
    let origin2 = "block:embedded-slash";
    create(&engine, origin2, &home, "buy milk/eggs", 1, false).await;
    let page2 = convert(&engine, origin2, "Home").await;
    assert_eq!(
        page2,
        PageId::for_page_under("Home", "buy milk/eggs")
            .unwrap()
            .as_str(),
        "an embedded '/' in the title must not spawn a 'buy milk' > 'eggs' page hierarchy"
    );
    assert!(is_page(&engine, &page2).await);
}

/// Read a block's stored `content` column.
async fn content_of(engine: &BackendEngine, id: &str) -> Option<String> {
    scalar(
        engine,
        &format!("SELECT content FROM {BLOCK_WRITE_TABLE} WHERE id = '{id}'"),
        "content",
    )
    .await
}

/// The `.org` file path `FileSyncController::authoritative_name_chain` would
/// form for a page: destination segments + the page's own `title()` as the
/// leaf, `.org` extension. `chain.push(block.title())` is the sole title
/// carrier on disk — the vault convention is filename-as-title (zero `.org`
/// files carry `#+TITLE:`; parser.rs derives the page title from the file stem
/// and the PBT model normalizes on it, so an emitted `#+TITLE:` would break
/// correspondence).
fn page_file_path(root: &Path, destination_segments: &[&str], page_title: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for seg in destination_segments {
        p = p.join(seg);
    }
    p.join(page_title).with_extension("org")
}

/// Round-trip invariant: convert → render the new page's own `.org` file →
/// reingest it → the reingested page title EQUALS the DB page title. Because
/// the title rides the FILENAME (no `#+TITLE:`), any character the filename
/// cannot carry verbatim breaks the round-trip. A trailing `/` is retained in
/// the DB title but stripped by path normalization when the filename is formed,
/// so an unsanitized title reingests DIVERGENT.
async fn assert_title_round_trips(origin_content: &str) {
    let engine = block_engine().await;
    let origin = "block:rt-origin";
    create(
        &engine,
        origin,
        "sentinel:no_parent",
        origin_content,
        0,
        false,
    )
    .await;
    let page = convert(&engine, origin, "").await;

    // The authoritative title the writeback must persist.
    let db_content = content_of(&engine, &page).await.expect("page content");
    let db_title = db_content.lines().next().unwrap_or("").to_string();

    // Reconstruct the page doc-root as file_sync renders it, then reingest.
    let bare = page.strip_prefix("block:").unwrap_or(&page);
    let doc_id = EntityUri::block(bare);
    let root_parent = EntityUri::from_raw("sentinel:no_parent");
    let mut doc = Block::new_text(doc_id.clone(), root_parent.clone(), db_content.clone());
    doc.set_page(true);

    let root = PathBuf::from("/vault");
    let path = page_file_path(&root, &[], &doc.title());
    let rendered = OrgRenderer::render_document(&doc, &[], &path, &doc_id);

    // Lock the vehicle decision: the title rides the filename, NEVER `#+TITLE:`.
    assert!(
        !rendered.contains("#+TITLE"),
        "page files carry the title in the FILENAME, not `#+TITLE:` (rendered: {rendered:?})"
    );

    let reparsed =
        parse_org_file(&path, &rendered, &root_parent, &root).expect("reingest the page file");
    assert_eq!(
        reparsed.document.title(),
        db_title,
        "convert writeback must persist the page title across render→reingest \
         (page file: {path:?}, rendered: {rendered:?})"
    );
}

/// Control: a clean title round-trips through the filename with no `#+TITLE:`.
#[tokio::test(flavor = "multi_thread")]
async fn convert_title_round_trips_clean() {
    assert_title_round_trips("Round Trip Clean").await;
}

/// RED (pre-fix): a trailing `/` in the title is kept in the DB but dropped by
/// path normalization, so the page reingests with a different title. The fix
/// sanitizes the trailing `/` at the convert boundary so title, id, and
/// filename all agree.
#[tokio::test(flavor = "multi_thread")]
async fn convert_title_round_trips_trailing_slash() {
    assert_title_round_trips("Trailing Slash/").await;
}

trait AssertApplied {
    fn assert_applied(self);
}
impl AssertApplied for UndoOutcome {
    fn assert_applied(self) {
        assert_eq!(self, UndoOutcome::Applied, "undo must apply");
    }
}
