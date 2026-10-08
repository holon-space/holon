//! Integration test for the engine-level `create_page_from_link` compound.
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

use holon::api::backend_engine::BackendEngine;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_WRITE_TABLE;
use holon::storage::turso::DbHandle;
use holon_api::EntityName;
use holon_api::EntityRef;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_api::block::Block;
use holon_core::OperationProvider;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;
use proptest::prelude::*;

const ENTITY: &str = "block";

async fn block_engine() -> Arc<BackendEngine> {
    create_test_engine_with_providers(":memory:".into(), |module| {
        module
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle,
                    BLOCK_WRITE_TABLE.to_string(),
                    ENTITY.to_string(),
                    ENTITY.to_string(),
                    BlockSchemaModule.edge_fields(),
                )) as Arc<dyn OperationProvider>
            })
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                let sql_ops = Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle.clone(),
                    BLOCK_WRITE_TABLE.to_string(),
                    ENTITY.to_string(),
                    ENTITY.to_string(),
                    BlockSchemaModule.edge_fields(),
                ));
                let mut block_raw_type_def = Block::type_definition();
                block_raw_type_def.name = BLOCK_WRITE_TABLE.to_string();
                let cache = tokio::task::block_in_place(|| {
                    // ALLOW(block_on): sync provider-factory closure on a multi_thread runtime.
                    tokio::runtime::Handle::current()
                        .block_on(QueryableCache::<Block>::new(db_handle, block_raw_type_def))
                })
                .expect("block_raw cache");
                Arc::new(SqlBlockOperations::new(sql_ops, Arc::new(cache)))
                    as Arc<dyn OperationProvider>
            })
    })
    .await
    .expect("test engine with the SqlOnly block providers")
}

/// A fixture write, outside the undo history.
async fn fixture_op(engine: &BackendEngine, op: &str, params: holon_api::StorageEntity) {
    engine
        .execute_operation(&EntityName::new(ENTITY), op, params, OpOrigin::Sync)
        .await
        .unwrap_or_else(|e| panic!("fixture {op}: {e:#}"));
}

async fn create_page_from_link(engine: &BackendEngine, target: &str) -> anyhow::Result<String> {
    let mut op_params: holon_api::StorageEntity = HashMap::new();
    op_params.insert("target".into(), Value::String(target.to_string()));
    let outcome = engine
        .execute_operation(
            &EntityName::new(ENTITY),
            "create_page_from_link",
            op_params,
            OpOrigin::User,
        )
        .await?;
    match outcome.response {
        Some(Value::String(s)) => Ok(s),
        other => panic!("expected the leaf page id in the response, got: {other:?}"),
    }
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
    p.insert(
        "parent_id".into(),
        Value::String("sentinel:no_parent".to_string()),
    );
    p
}

async fn link_resolved(handle: &DbHandle, source: &str) -> Option<String> {
    let sql = format!(
        "SELECT resolved_id FROM block_links WHERE source_block_id = 'block:{source}' AND kind = \
         'page'"
    );
    let rows = handle.query(&sql, HashMap::new()).await.expect("query");
    rows.into_iter()
        .next()
        .and_then(|r| match r.get("resolved_id") {
            Some(Value::String(s)) => Some(s.clone()),
            _ => None,
        })
}

async fn block_content(handle: &DbHandle, id: &str) -> Option<String> {
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

async fn block_parent(handle: &DbHandle, id: &str) -> Option<String> {
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

async fn block_has_page_tag(handle: &DbHandle, id: &str) -> bool {
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

async fn block_count(handle: &DbHandle) -> usize {
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
    let engine = block_engine().await;
    let handle = engine.db_handle();

    // 1. Create a source block with a dangling [[Projects/X]] link.
    let mut p = create_params("src", "see [[Projects/X]] for more");
    p.insert(
        "marks".into(),
        Value::String(name_link_marks("Projects/X", 4, 15)),
    );
    fixture_op(&engine, "create", p).await;

    // Verify the link is dangling initially.
    assert!(
        link_resolved(handle, "src").await.is_none(),
        "link should be dangling before create_page_from_link"
    );

    let initial_block_count = block_count(handle).await;

    // 2. Invoke create_page_from_link("Projects/X").
    let leaf_id = create_page_from_link(&engine, "Projects/X")
        .await
        .expect("create_page_from_link");
    assert!(leaf_id.starts_with("block:"), "leaf id must be a block URI");

    // 3. Assert: `Projects` page exists, Page-tagged, at the top-level parent.
    let projects_sql = "SELECT id FROM block_raw b JOIN block_tags t ON t.block_id = b.id AND \
                        t.tag = 'Page' WHERE b.content = 'Projects'";
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
    let parent = block_parent(handle, &projects_id).await;
    assert_eq!(
        parent.as_deref(),
        Some("sentinel:no_parent"),
        "Projects page must be top-level (sentinel:no_parent), got: {parent:?}"
    );
    assert!(
        block_has_page_tag(handle, &projects_id).await,
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
        block_has_page_tag(handle, &x_id).await,
        "X must be Page-tagged"
    );
    assert_eq!(
        block_content(handle, &x_id).await.as_deref(),
        Some("X"),
        "X block content must be 'X'"
    );

    // 5. Assert: the source block's dangling link is now healed (resolved_id = the
    //    X page).
    let resolved = link_resolved(handle, "src").await;
    assert_eq!(
        resolved.as_deref(),
        Some(x_id.as_str()),
        "link must be healed to point at X page"
    );

    // 6. Idempotency: running again returns the same leaf id, no new blocks.
    let block_count_after_first = block_count(handle).await;
    let leaf_id2 = create_page_from_link(&engine, "Projects/X")
        .await
        .expect("second create_page_from_link");
    assert_eq!(
        leaf_id2, leaf_id,
        "second invocation must return the same leaf id"
    );
    assert_eq!(
        block_count(handle).await,
        block_count_after_first,
        "second invocation must not create new blocks"
    );
    assert_eq!(
        block_count(handle).await,
        initial_block_count + 2,
        "only two new blocks (Projects + X) should have been created"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn create_page_from_link_empty_target_is_error() {
    let engine = block_engine().await;
    let result = create_page_from_link(&engine, "").await;
    assert!(result.is_err(), "empty target must produce an error");
}

// ---------------------------------------------------------------------------
// inv-page-name-unique (cross-peer convergence PBT)
// ---------------------------------------------------------------------------
//
// Holon pages live on a CRDT (Loro) substrate. A page has NO independent
// identity beyond its (normalized name, position): two peers that each create
// "the Areas page" are creating the SAME logical entity, so they MUST mint the
// SAME block id. If they don't, a later merge — the ids are the primary key,
// so the merge is a union by id — keeps BOTH blocks, and the vault now carries
// two Page-tagged blocks named "Areas".
//
// This property drives `create_page_from_link`, the path a `[[Areas]]` click
// takes when the page doesn't exist yet, on two INDEPENDENT peers (each its own
// store) for a generated page name and asserts the minted leaf ids converge.
// Because block ids are the CRDT merge key, `id_a == id_b` is precisely the
// condition under which the merged vault holds ONE page named `name`.

/// A well-formed page path: 1–3 non-empty segments joined by a `/` that may
/// carry surrounding spaces (`"Areas / Sub"`). The multi-segment + spaced-
/// separator shapes exercise the H2 canonicalization case — the parser trims
/// segments (`normalize_for_hash` alone would not) so its optimistic id agrees
/// with the id the writer mints. Shrinking still drives to a minimal witness.
fn page_name_strategy() -> impl Strategy<Value = String> {
    let segment = "[A-Za-z][A-Za-z0-9 ]{0,7}"
        .prop_map(|s| s.trim().to_string())
        .prop_filter("segment must be non-empty after trimming", |s| {
            !s.is_empty()
        });
    let separator = prop_oneof![
        Just("/".to_string()),
        Just(" / ".to_string()),
        Just("/ ".to_string()),
        Just(" /".to_string()),
    ];
    (proptest::collection::vec(segment, 1..=3), separator)
        .prop_map(|(segments, sep)| segments.join(&sep))
}

/// Create page `name` via `create_page_from_link` on a fresh, independent peer
/// (its own in-memory store) and return the minted leaf id.
async fn create_page_on_fresh_peer(name: &str) -> String {
    let engine = block_engine().await;
    create_page_from_link(&engine, name)
        .await
        .expect("create_page_from_link")
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 24,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// inv-page-name-unique: independent peers that each create the same-named
    /// page must converge on one page identity, so a merge yields no duplicate.
    ///
    /// Page identity is a deterministic function of the normalized path
    /// (`PageId::for_path`, minted by every write path), so two independent
    /// peers that each create page `name` mint the SAME block id and a merge
    /// yields ONE page. RULING: path-hash for new writes + bounded
    /// `(name,parent)` repair — see docs/Plans/PageIdentityDeterminism.md.
    #[test]
    fn inv_page_name_unique_converges_across_peers(name in page_name_strategy()) {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        let (id_a, id_b) = rt.block_on(async {
            let a = create_page_on_fresh_peer(&name).await;
            let b = create_page_on_fresh_peer(&name).await;
            (a, b)
        });

        prop_assert_eq!(
            &id_a,
            &id_b,
            "inv-page-name-unique: two independent peers each created page {:?} but minted \
             divergent block ids ({} vs {}). Block ids are the CRDT merge key, so on merge the \
             vault holds TWO Page-tagged blocks named {:?} — the duplicate-page bug. Page \
             identity must be a deterministic function of the (normalized name, position), not a \
             random UUID.",
            name, id_a, id_b, name
        );

        // H2 guard: the link PARSER's optimistic target id for the same raw
        // target must equal the id the WRITER minted. Otherwise a click's
        // healed `resolved_id` would point at a different id than the page that
        // gets created — divergence that only the name-based re-resolve trigger
        // papers over. This directly exercises spaced separators ("Areas / Sub")
        // where the parser trims segments to converge with the writer.
        if let holon_api::link_parser::LinkTarget::CreationIntent {
            scheme, target_id, ..
        } = holon_api::link_parser::LinkTargetClassifier::default().classify(&name)
            && scheme == "block"
        {
            prop_assert_eq!(
                target_id.as_str(),
                id_a.as_str(),
                "parser/writer page-id divergence for target {:?}: parser optimistic id {} != \
                 writer-minted id {}",
                name,
                target_id.as_str(),
                id_a
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Page-id collision after rename
// ---------------------------------------------------------------------------
//
// `PageId::for_path` mints a page's id as blake3(normalized path). Per
// docs/Plans/PageIdentityDeterminism.md §5.3 a RENAME is "an ordinary edit to
// the existing entity — the id does NOT re-mint", and "a *new* page created
// later under the new name gets a new id; that is correct (it is a different
// logical page)".
//
// This test executes exactly that sequence:
//   1. create page "A"          → id = H("A")
//   2. rename A → B (content edit on the same entity, id unchanged)
//   3. create page "A" again    → minting recomputes H("A") … already taken
//
// The assertions state what §5.3 PROMISES: step 3 yields a DIFFERENT entity
// than step 1, and two distinct pages ("B" and "A") coexist. The interim
// ADR 0029 D1b policy refuses step 3 with `IdentityCollision` instead.

/// Create a `Page`-tagged block with an explicit id/content/parent.
async fn create_page(engine: &BackendEngine, id: &str, content: &str, parent: &str) {
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String(id.to_string()));
    p.insert("content".into(), Value::String(content.to_string()));
    p.insert("parent_id".into(), Value::String(parent.to_string()));
    p.insert(
        "tags".into(),
        Value::Array(vec![Value::String("Page".to_string())]),
    );
    fixture_op(engine, "create", p).await;
}

/// Clicking a NAME-form link whose page already exists navigates to THAT page
/// and creates nothing.
///
/// This is the whole reason write-back may keep the authored `[[Journals]]`
/// bytes (task #32, ruling B): navigation resolves the name at click time,
/// through the same `resolve_page_name` the junction uses, so the file never
/// has to carry the id.
#[tokio::test(flavor = "multi_thread")]
async fn create_page_from_link_navigates_to_an_existing_page_without_creating_one() {
    let engine = block_engine().await;
    let handle = engine.db_handle();

    create_page(&engine, "block:journals", "Journals", "sentinel:no_parent").await;

    let mut p = create_params("src", "see Journals now");
    p.insert(
        "marks".into(),
        Value::String(name_link_marks("Journals", 4, 12)),
    );
    fixture_op(&engine, "create", p).await;
    assert_eq!(
        link_resolved(handle, "src").await.as_deref(),
        Some("block:journals"),
        "a name link to an existing page resolves in the junction at write time"
    );

    let before = block_count(handle).await;
    let leaf = create_page_from_link(&engine, "Journals")
        .await
        .expect("create_page_from_link");

    assert_eq!(
        leaf, "block:journals",
        "navigation must land on the EXISTING page, not a freshly minted one"
    );
    assert_eq!(
        block_count(handle).await,
        before,
        "no block may be created when the target page already exists"
    );
}

/// Rename a page: an ordinary content edit on the existing entity (§5.3).
async fn rename_page(engine: &BackendEngine, id: &str, new_content: &str) {
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String(id.to_string()));
    p.insert("content".into(), Value::String(new_content.to_string()));
    fixture_op(engine, "update", p).await;
}

async fn page_rows(handle: &DbHandle) -> Vec<(String, String)> {
    let sql = "SELECT b.id AS id, b.content AS content FROM block_raw b JOIN block_tags t ON \
               t.block_id = b.id AND t.tag = 'Page' ORDER BY b.id";
    handle
        .query(sql, HashMap::new())
        .await
        .expect("query pages")
        .into_iter()
        .map(|r| {
            (
                r.get("id").and_then(|v| v.as_string()).unwrap().to_string(),
                r.get("content")
                    .and_then(|v| v.as_string())
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "ADR 0029 D1b end-state pending: unique-random recreate not implemented"]
async fn recreating_a_renamed_pages_old_name_yields_a_distinct_page() {
    let engine = block_engine().await;
    let handle = engine.db_handle();

    // 1. Create page A via the production lazy-create op.
    let id_a = create_page_from_link(&engine, "A")
        .await
        .expect("create page A");

    // A child under A, so we can see whether it follows the renamed entity.
    create_page(&engine, "block:childOfA", "Child", &id_a).await;

    // 2. Rename A → B. Same entity, same id (§5.3).
    rename_page(&engine, &id_a, "B").await;
    assert_eq!(
        block_content(handle, &id_a).await.as_deref(),
        Some("B"),
        "after rename the entity {id_a} must be titled B"
    );

    // 3. Create a NEW page A.
    let id_a2 = create_page_from_link(&engine, "A").await.expect(
        "recreating page A must succeed (§5.3). Interim ADR 0029 D1b refuses it with \
         IdentityCollision instead; the end-state unique-random recreate is not implemented",
    );

    let pages = page_rows(handle).await;

    assert_ne!(
        id_a2, id_a,
        "§5.3: the newly created page A must be a DIFFERENT entity than the page that was renamed \
         to B, but both are {id_a}. Observed pages after the sequence: {pages:?}"
    );

    let titles: Vec<&str> = pages.iter().map(|(_, c)| c.as_str()).collect();
    assert!(
        titles.contains(&"B") && titles.contains(&"A"),
        "both the renamed page B and the new page A must exist; observed pages: {pages:?}"
    );

    // The child was created under the page that is now titled "B". Asserting only
    // that its parent is still `id_a` would pass VACUOUSLY under the defect: the
    // collision leaves exactly one entity, so `parent == id_a` holds whether that
    // entity is the surviving "B" or the clobbered "A". Assert on the parent's
    // TITLE, which is what actually distinguishes the two worlds.
    let child_parent = block_parent(handle, "block:childOfA")
        .await
        .expect("child must still have a parent");
    let child_parent_title = block_content(handle, &child_parent).await;
    assert_eq!(
        child_parent_title.as_deref(),
        Some("B"),
        "the child must still hang under the RENAMED page B, but its parent \
         {child_parent} is titled {child_parent_title:?} — the new page A overwrote the \
         renamed entity instead of becoming a distinct one; observed pages: {pages:?}"
    );
}
