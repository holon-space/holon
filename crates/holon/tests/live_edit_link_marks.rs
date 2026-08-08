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

use holon::api::AuthoredInput;
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
use holon_core::OperationWrapper;
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

fn sql_provider(handle: DbHandle) -> Arc<SqlOperationProvider> {
    Arc::new(SqlOperationProvider::with_edge_fields(
        handle,
        TABLE.to_string(),
        ENTITY.to_string(),
        ENTITY.to_string(),
        vec![tags_descriptor()],
    ))
}

fn dispatcher(handle: DbHandle) -> OperationDispatcher {
    OperationDispatcher::new(vec![sql_provider(handle) as Arc<dyn OperationProvider>])
}

/// Sync target for the wrapper's type parameter. The prod SqlOnly wiring hands
/// `OperationWrapper` a real org sync provider; nothing here asserts on sync,
/// so the wrapper is built in passthrough mode with this as the phantom.
struct NoSync;

#[async_trait::async_trait]
impl holon_core::traits::SyncableProvider for NoSync {
    fn provider_name(&self) -> &str {
        "no-sync"
    }

    async fn sync(
        &self,
        _: holon_api::StreamPosition,
    ) -> holon_core::traits::Result<holon_api::StreamPosition> {
        unreachable!("no test drives sync")
    }

    async fn sync_changes(
        &self,
        _: &[holon_core::traits::FieldDelta],
    ) -> holon_core::traits::Result<()> {
        Ok(())
    }
}

/// The dispatcher as the SqlOnly composition root actually builds it: the SQL
/// CRUD authority behind an [`OperationWrapper`] (`turso_seams.rs`), which is
/// the member the operation registry holds. Anything the wrapper fails to
/// forward is invisible to the dispatcher.
fn wrapped_dispatcher(handle: DbHandle) -> OperationDispatcher {
    let wrapper: OperationWrapper<NoSync> =
        OperationWrapper::without_sync(sql_provider(handle) as Arc<dyn OperationProvider>);
    OperationDispatcher::new(vec![Arc::new(wrapper) as Arc<dyn OperationProvider>])
}

/// Both helpers dispatch as `AuthoredInput::Live` — they stand in for a person
/// typing, which is what the engine declares for `OpOrigin::User`/`Agent`.
/// Tests that must model a replayed inverse call `execute_operation` directly,
/// exactly as `OperationEngine::replay` does.
async fn create_block(d: &OperationDispatcher, entity: &EntityName, id: &str, content: &str) {
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String(format!("block:{id}")));
    p.insert("content".into(), Value::String(content.to_string()));
    d.execute_operation_with_input(entity, "create", p, AuthoredInput::Live)
        .await
        .expect("create block");
}

async fn set_content(d: &OperationDispatcher, entity: &EntityName, id: &str, content: &str) {
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String(format!("block:{id}")));
    p.insert("field".into(), Value::String("content".to_string()));
    p.insert("value".into(), Value::String(content.to_string()));
    d.execute_operation_with_input(entity, "set_field", p, AuthoredInput::Live)
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

/// Task #23 — the SAME link removal, driven through the wiring PROD actually
/// registers. `live_content_edit_removing_link_clears_junction` above passes a
/// BARE `SqlOperationProvider` to the dispatcher, so the ground-truth read
/// reaches the SQL row; the composition root wraps that provider, and a
/// wrapper that does not forward `read_block_content_marks` answers "I cannot
/// read marks". The dispatcher then takes its fail-safe branch (never null on
/// an unknown prior state) and the removed link's marks + junction row SURVIVE
/// the edit — stale marks pointing into text that no longer holds a link.
#[tokio::test(flavor = "multi_thread")]
async fn removing_a_link_clears_marks_through_the_wrapped_authority() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;
    let d = wrapped_dispatcher(handle.clone());
    let entity: EntityName = ENTITY.to_string().into();

    create_block(&d, &entity, "src", "").await;
    set_content(&d, &entity, "src", "[[Gone Page]]").await;
    let (_, marks) = read_content_marks(&handle, "src").await;
    assert!(marks.is_some(), "the link commit must populate marks");

    set_content(&d, &entity, "src", "just plain text now").await;

    let (content, marks) = read_content_marks(&handle, "src").await;
    assert_eq!(content, "just plain text now");
    assert_eq!(
        marks, None,
        "removing the link must clear marks even when the CRUD authority sits behind the \
         registered OperationWrapper"
    );
    assert_eq!(
        links_rows(&handle, "src").await,
        vec![],
        "the junction row must go with the marks"
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

/// The CREATE half of the same boundary — the vector dogfood 2026-08-08 P1-2
/// caught. Typing a link into the creation slot commits through `block.create`,
/// not `set_field`, so a block can be BORN carrying raw markup. It must be
/// adopted at that boundary exactly like an edit: stripped label in `content`,
/// the `Link` mark in `marks`, a junction row, and a backlink once the target
/// page exists.
#[tokio::test(flavor = "multi_thread")]
async fn creating_a_block_with_a_typed_link_adopts_it_at_the_write_boundary() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;
    let d = dispatcher(handle.clone());
    let entity: EntityName = ENTITY.to_string().into();

    // The user types the whole line into the creation slot and presses Enter.
    create_block(&d, &entity, "src", "see [[Linked Page Test]] now").await;

    let (content, marks) = read_content_marks(&handle, "src").await;
    assert_eq!(
        content, "see Linked Page Test now",
        "a born-with-a-link block must store the rendered label, not the raw [[...]] syntax \
         (storing it raw is what the next boot's re-ingest silently rewrites)"
    );
    let marks = marks.expect("marks must be populated by the create boundary (was NULL — the bug)");
    let parsed: Vec<MarkSpan> = holon_api::marks_from_json(&marks).expect("marks JSON round-trips");
    assert_eq!(parsed.len(), 1, "one link mark");
    assert!(
        matches!(parsed[0].mark, InlineMark::Link { .. }),
        "the extracted mark is a Link"
    );
    assert_eq!(
        links_rows(&handle, "src").await,
        vec![("Linked Page Test".to_string(), "page".to_string(), None)],
        "a created wiki-name link must produce a dangling page-kind junction row"
    );

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
        "backlinks matview must show the link the block was born with"
    );
}

/// The adoption is a FIXED POINT: re-ingesting what the create boundary stored
/// leaves it byte-identical. This is the half that kills the silent rewrite —
/// the next boot re-parses the file the renderer wrote and finds nothing to
/// change, so the user's characters stay put.
#[tokio::test(flavor = "multi_thread")]
async fn adopted_create_content_survives_a_re_ingest_unchanged() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;
    let d = dispatcher(handle.clone());
    let entity: EntityName = ENTITY.to_string().into();

    create_block(&d, &entity, "src", "see [[Linked Page Test]] now").await;
    let (content, marks) = read_content_marks(&handle, "src").await;
    let spans: Vec<MarkSpan> =
        holon_api::marks_from_json(&marks.expect("marks present")).expect("marks JSON");

    // What the org writer puts on disk, and what re-reading it yields.
    let on_disk = holon_org_format::render_inline_marks(&content, &spans);
    let (reparsed, reparsed_marks) = holon_org_format::extract_inline_marks(&on_disk);
    assert_eq!(
        reparsed, content,
        "a re-ingest of the rendered file must not change the stored content"
    );
    assert_eq!(
        reparsed_marks, spans,
        "a re-ingest of the rendered file must not change the stored marks"
    );
}

/// The no-clobber contract: a caller that supplies its OWN `marks` has already
/// parsed at its own boundary (org ingest's `build_block_params`,
/// `split_block`'s partitioned spans). Those creates reach this same
/// dispatcher, so re-parsing them here would fight the parse that produced
/// them.
#[tokio::test(flavor = "multi_thread")]
async fn a_create_that_supplies_marks_is_not_reparsed() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;
    let d = dispatcher(handle.clone());
    let entity: EntityName = ENTITY.to_string().into();

    // The ingest shape: an already-stripped label whose surviving `[[...]]` bytes
    // are sealed as literal by a Verbatim mark covering them.
    let sealed = vec![MarkSpan::new(4, 17, InlineMark::Verbatim)];
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String("block:src".to_string()));
    p.insert(
        "content".into(),
        Value::String("see [[Not A Link]] now".to_string()),
    );
    p.insert(
        "marks".into(),
        Value::String(holon_api::marks_to_json(&sealed)),
    );
    d.execute_operation_with_input(&entity, "create", p, AuthoredInput::Live)
        .await
        .expect("create with caller-supplied marks");

    let (content, marks) = read_content_marks(&handle, "src").await;
    assert_eq!(
        content, "see [[Not A Link]] now",
        "a caller that supplied marks keeps its content verbatim"
    );
    let parsed: Vec<MarkSpan> =
        holon_api::marks_from_json(&marks.expect("supplied marks kept")).expect("marks JSON");
    assert_eq!(parsed, sealed, "the supplied mark set must survive intact");
    assert_eq!(
        links_rows(&handle, "src").await,
        vec![],
        "a sealed literal must not be adopted into a junction row"
    );
}

/// The inverse of a delete must restore the deleted BYTES, even when those
/// bytes are markup the create boundary would otherwise adopt.
///
/// The trap this locks shut: `capture_row` filters `Value::Null` columns
/// (`sql_operation_provider.rs`), so the delete-inverse of a marks-NULL row
/// carries NO `marks` key at all — the shape that also arrives from raw
/// pre-adoption blocks and from ingest. "No marks param" is therefore NOT
/// evidence of "unparsed live typing", and adoption keyed on its absence
/// rewrites the user's block during UNDO: `"see [[Journals]] now"` comes back
/// as `"see Journals now"` with minted marks and a junction row, breaking the
/// identity-preserving inverse contract (ADR 0024). Adoption is gated on the
/// authoring ORIGIN instead, and `OperationEngine::replay` dispatches without
/// one — so this replay path cannot reach it by construction.
#[tokio::test(flavor = "multi_thread")]
async fn undo_of_a_delete_restores_bytes_verbatim_even_when_adoption_would_apply() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;
    let provider = sql_provider(handle.clone());
    let d = dispatcher(handle.clone());
    let entity: EntityName = ENTITY.to_string().into();

    // A pre-adoption / ingest-shaped row: raw markup, NULL marks, written
    // straight to the SQL authority the way org ingest writes it.
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String("block:legacy".to_string()));
    p.insert(
        "content".into(),
        Value::String("see [[Journals]] now".to_string()),
    );
    provider
        .execute_operation(&entity, "create", p)
        .await
        .expect("seed legacy row");
    let before = read_content_marks(&handle, "legacy").await;
    assert_eq!(before.0, "see [[Journals]] now", "seeded verbatim");
    assert_eq!(before.1, None, "seeded with NULL marks");

    let mut dp: holon_api::StorageEntity = HashMap::new();
    dp.insert("id".into(), Value::String("block:legacy".to_string()));
    let res = d
        .execute_operation(&entity, "delete", dp)
        .await
        .expect("delete");
    let inverse = match res.undo {
        holon_core::traits::UndoAction::Undo(op) => op,
        other => panic!("leaf delete must be reversible, got {other:?}"),
    };
    assert_eq!(inverse.op_name, "create");
    assert!(
        !inverse.params.contains_key("marks"),
        "capture_row drops NULL columns, so the inverse carries no marks param — this is the \
         shape that must NOT be read as 'unparsed user typing'"
    );

    // Replay it exactly as `OperationEngine::replay` does: straight through the
    // dispatcher, carrying no origin.
    d.execute_operation(
        &inverse.entity_name,
        &inverse.op_name,
        inverse
            .params
            .iter()
            .map(|(k, v)| (std::sync::Arc::from(k.as_str()), v.clone()))
            .collect(),
    )
    .await
    .expect("replay inverse");

    let after = read_content_marks(&handle, "legacy").await;
    assert_eq!(
        after.0, before.0,
        "undo must restore the deleted bytes verbatim"
    );
    assert_eq!(after.1, before.1, "undo must not mint marks");
    assert_eq!(
        links_rows(&handle, "legacy").await,
        vec![],
        "undo must not mint a junction row"
    );
}

/// The same trap on the EDIT arm, which has adopted since links-increment-3:
/// the inverse of a content edit carries the block's PREVIOUS content, and for
/// a pre-adoption block that previous content is raw markup. Undo must put
/// those bytes back, not adopt them.
///
/// IGNORED, and the reason is a real fork rather than a missing line. The edit
/// arm cannot simply be origin-gated like the create arm: `capture_row` filters
/// NULL columns, so a content inverse cannot say "restore marks to NULL", and
/// what actually clears stale marks on undo today is the adoption follow-up
/// firing during replay (`undo_link_add_restores_prior_pair` in
/// `undo_marks_consistency_repro.rs` fails the moment the arm is gated —
/// measured, not predicted). Undo is therefore correct for adopted blocks and
/// wrong for raw ones, and picking either behaviour is picking which population
/// to break. The fix both tests would pass is inverses that carry their marks
/// explicitly, i.e. `capture_row` emitting explicit NULLs — a change to every
/// inverse in the system. Escalated in `lane-report-12.md`; un-ignore this test
/// in the lane that makes that change.
#[ignore = "blocked on inverses that carry marks explicitly (capture_row NULL filtering); see \
            lane-report-12.md"]
#[tokio::test(flavor = "multi_thread")]
async fn undo_of_a_content_edit_restores_raw_previous_bytes() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;
    let provider = sql_provider(handle.clone());
    let d = dispatcher(handle.clone());
    let entity: EntityName = ENTITY.to_string().into();

    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String("block:legacy".to_string()));
    p.insert(
        "content".into(),
        Value::String("see [[Journals]] now".to_string()),
    );
    provider
        .execute_operation(&entity, "create", p)
        .await
        .expect("seed legacy row");

    let mut sp: holon_api::StorageEntity = HashMap::new();
    sp.insert("id".into(), Value::String("block:legacy".to_string()));
    sp.insert("field".into(), Value::String("content".to_string()));
    sp.insert("value".into(), Value::String("replaced".to_string()));
    let res = d
        .execute_operation(&entity, "set_field", sp)
        .await
        .expect("edit");
    let inverse = match res.undo {
        holon_core::traits::UndoAction::Undo(op) => op,
        other => panic!("content edit must be reversible, got {other:?}"),
    };

    d.execute_operation(
        &inverse.entity_name,
        &inverse.op_name,
        inverse
            .params
            .iter()
            .map(|(k, v)| (std::sync::Arc::from(k.as_str()), v.clone()))
            .collect(),
    )
    .await
    .expect("replay inverse");

    let after = read_content_marks(&handle, "legacy").await;
    assert_eq!(
        after.0, "see [[Journals]] now",
        "undo of an edit must restore the previous bytes verbatim"
    );
    assert_eq!(after.1, None, "undo of an edit must not mint marks");
    assert_eq!(links_rows(&handle, "legacy").await, vec![]);
}

/// A page named `name` with id `block:<id>`, so wiki-name links to it resolve.
async fn create_page(d: &OperationDispatcher, entity: &EntityName, id: &str, name: &str) {
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String(format!("block:{id}")));
    p.insert("content".into(), Value::String(name.to_string()));
    p.insert(
        "tags".into(),
        Value::Array(vec![Value::String("Page".to_string())]),
    );
    d.execute_operation(entity, "create", p)
        .await
        .expect("create page");
}

/// Ruling B — write-back emits the AUTHORED bytes, so re-ingesting its output
/// is a fixed point for a RESOLVED link too.
///
/// The link resolves in `block_links` the moment it is typed, but the file
/// keeps `[[Journals]]`. Feeding those bytes back through the write boundary
/// (what the next boot's ingest does) leaves the `Name` mark and the
/// `kind='page'` junction row exactly as they were — no `page`→`block` flip, so
/// store and disk never disagree about the same link.
///
/// The bytes are the stored marks through `render_inline_marks`, which is what
/// write-back emits: that `WritebackRenderer` applies no further transform is
/// pinned by `holon-orgmode/tests/writeback_emits_authored_link_bytes.rs`.
#[tokio::test(flavor = "multi_thread")]
async fn a_resolved_name_link_re_ingests_to_the_same_mark_and_junction() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;
    let d = dispatcher(handle.clone());
    let entity: EntityName = ENTITY.to_string().into();

    create_page(&d, &entity, "journals", "Journals").await;
    create_block(&d, &entity, "src", "see [[Journals]] now").await;

    let (content, marks) = read_content_marks(&handle, "src").await;
    let spans: Vec<MarkSpan> =
        holon_api::marks_from_json(&marks.expect("marks present")).expect("marks JSON");
    let junction = links_rows(&handle, "src").await;
    assert_eq!(
        junction,
        vec![(
            "Journals".to_string(),
            "page".to_string(),
            Some("block:journals".to_string())
        )],
        "typing a link to an EXISTING page must resolve in the junction immediately"
    );

    let on_disk = holon_org_format::render_inline_marks(&content, &spans);
    assert_eq!(
        on_disk, "see [[Journals]] now",
        "write-back must put the authored name form on disk, not the resolved id"
    );

    // The next boot re-ingests those bytes through the write boundary.
    set_content(&d, &entity, "src", &on_disk).await;

    let (content_after, marks_after) = read_content_marks(&handle, "src").await;
    assert_eq!(content_after, content, "re-ingest must not change content");
    let spans_after: Vec<MarkSpan> =
        holon_api::marks_from_json(&marks_after.expect("marks survive the re-ingest"))
            .expect("marks JSON");
    assert_eq!(spans_after, spans, "re-ingest must not rewrite the mark");
    assert_eq!(
        links_rows(&handle, "src").await,
        junction,
        "re-ingest must not flip the junction row from page-kind to block-kind"
    );
}

/// The input side is untouched: a file already holding the RESOLVED form —
/// legal authored input, and what every file written before ruling B carries —
/// still ingests as a resolved block-kind link, and is its own fixed point.
#[tokio::test(flavor = "multi_thread")]
async fn a_resolved_form_link_on_disk_still_ingests_as_a_block_link() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;
    let d = dispatcher(handle.clone());
    let entity: EntityName = ENTITY.to_string().into();

    create_page(&d, &entity, "journals", "Journals").await;
    create_block(&d, &entity, "src", "see [[block:journals][Journals]] now").await;

    let (content, marks) = read_content_marks(&handle, "src").await;
    assert_eq!(content, "see Journals now");
    let spans: Vec<MarkSpan> =
        holon_api::marks_from_json(&marks.expect("marks present")).expect("marks JSON");
    assert_eq!(
        spans[0].mark,
        InlineMark::Link {
            target: holon_api::EntityRef::Scheme {
                raw: "block:journals".to_string()
            },
            label: "Journals".to_string(),
        },
        "an id-form link keeps its authored id target"
    );
    assert_eq!(
        links_rows(&handle, "src").await,
        vec![(
            "block:journals".to_string(),
            "block".to_string(),
            Some("block:journals".to_string())
        )],
        "an id-form link resolves trivially, exactly as before ruling B"
    );
    assert_eq!(
        holon_org_format::render_inline_marks(&content, &spans),
        "see [[block:journals][Journals]] now",
        "and it is its own write-back fixed point"
    );
}
