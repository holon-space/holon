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
use proptest::prelude::*;

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

// ---------------------------------------------------------------------------
// inv-page-name-unique (cross-peer convergence PBT)
// ---------------------------------------------------------------------------
//
// Reproduces the "two pages named X coexist in the sidebar" bug (user
// screenshot) as a property, root-cause first.
//
// Holon pages live on a CRDT (Loro) substrate. A page has NO independent
// identity beyond its (normalized name, position): two peers that each create
// "the Areas page" are creating the SAME logical entity, so they MUST mint the
// SAME block id. If they don't, a later merge — the ids are the primary key,
// so the merge is a union by id — keeps BOTH blocks, and the vault now carries
// two Page-tagged blocks named "Areas". That is exactly the duplicate the user
// saw.
//
// The SUT is the production lazy-page-creation op `create_page_from_link`,
// the path a `[[Areas]]` click/link-resolve takes when the page doesn't exist
// yet. Its within-store dedup (`resolve_page_name`, a name lookup) papers over
// duplicates as long as everything is in one already-merged store; it cannot
// see an unmerged peer's page, so it does NOT make the minted id a function of
// the name.
//
// This property drives the op on two INDEPENDENT peers (each its own store)
// for a generated page name and asserts the minted leaf ids converge. Because
// block ids are the CRDT merge key, `id_a == id_b` is precisely the condition
// under which the merged vault holds ONE page named `name` — i.e. asserting
// equality IS asserting inv-page-name-unique over the merged peer set. The ids
// come entirely from the SUT (only the name is generated), so the property
// checks the system, not the seed.
//
// RED today: `create_page_from_link` mints `uuid::Uuid::new_v4()`
// (sql_operation_provider.rs), so the two peers diverge and the property
// fails, shrinking to a minimal one-character name.

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

/// Create page `name` via the production `create_page_from_link` op on a fresh,
/// independent peer (its own in-memory store) and return the minted leaf id.
async fn create_page_on_fresh_peer(name: &str) -> String {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;
    let provider = provider(handle.clone());
    let entity: EntityName = ENTITY.to_string().into();

    let mut op_params: holon_api::StorageEntity = HashMap::new();
    op_params.insert("target".into(), Value::String(name.to_string()));
    let result = provider
        .execute_operation(&entity, "create_page_from_link", op_params)
        .await
        .expect("create_page_from_link");
    match result.response {
        Some(Value::String(s)) => s,
        other => panic!("expected leaf page id in response, got: {other:?}"),
    }
}

/// Create a `Page`-tagged block with an explicit id/content/parent.
async fn create_page(
    provider: &SqlOperationProvider,
    entity: &EntityName,
    id: &str,
    content: &str,
    parent: &str,
) {
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String(id.to_string()));
    p.insert("content".into(), Value::String(content.to_string()));
    p.insert("parent_id".into(), Value::String(parent.to_string()));
    p.insert(
        "tags".into(),
        Value::Array(vec![Value::String("Page".to_string())]),
    );
    provider
        .execute_operation(entity, "create", p)
        .await
        .expect("create page");
}

/// The bounded `(name, parent)` repair pass collapses pre-existing duplicate
/// pages onto the lowest-id survivor, re-homing children and inbound links.
#[tokio::test(flavor = "multi_thread")]
async fn dedup_pages_collapses_duplicates_onto_lowest_id_survivor() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;
    let provider = provider(handle.clone());
    let entity: EntityName = ENTITY.to_string().into();

    // Two "Areas" pages under the same (sentinel) parent — the duplicate the
    // user saw. `block:aaa` < `block:zzz`, so `aaa` is the survivor.
    let survivor = "block:aaa";
    let loser = "block:zzz";
    create_page(&provider, &entity, survivor, "Areas", "sentinel:no_parent").await;
    create_page(&provider, &entity, loser, "Areas", "sentinel:no_parent").await;
    // A child page living under the LOSER — must be re-homed to the survivor.
    create_page(&provider, &entity, "block:child", "Sub", loser).await;
    // An inbound link resolved to the LOSER — must be rewritten to the survivor.
    // Plus an OUTBOUND link FROM the loser — redundant with the survivor's, must
    // be deleted so it isn't orphaned when the loser's block_raw row is removed.
    handle
        .transaction(vec![
            (
                format!(
                    "INSERT INTO block_links (source_block_id, target, kind, resolved_id) VALUES \
                     ('block:src', 'Areas', 'page', '{loser}')"
                ),
                vec![],
            ),
            (
                format!(
                    "INSERT INTO block_links (source_block_id, target, kind, resolved_id) VALUES \
                     ('{loser}', 'Elsewhere', 'page', NULL)"
                ),
                vec![],
            ),
        ])
        .await
        .expect("seed inbound + outbound links");

    let removed = provider.dedup_pages().await.expect("dedup_pages");
    assert_eq!(removed, 1, "exactly one loser page removed");

    // Survivor kept, loser gone.
    assert!(
        block_has_page_tag(&handle, survivor).await,
        "survivor must remain Page-tagged"
    );
    assert_eq!(
        block_content(&handle, loser).await,
        None,
        "loser page row must be deleted"
    );
    assert!(
        !block_has_page_tag(&handle, loser).await,
        "loser Page tag must be deleted"
    );
    // Child re-homed and inbound link rewritten onto the survivor.
    assert_eq!(
        block_parent(&handle, "block:child").await.as_deref(),
        Some(survivor),
        "child must be re-homed under the survivor"
    );
    assert_eq!(
        link_resolved(&handle, "src").await.as_deref(),
        Some(survivor),
        "inbound link must resolve to the survivor"
    );
    // The loser's outbound link row is gone (not orphaned).
    let orphan_rows = handle
        .query(
            &format!("SELECT 1 FROM block_links WHERE source_block_id = '{loser}'"),
            HashMap::new(),
        )
        .await
        .expect("query outbound links");
    assert!(
        orphan_rows.is_empty(),
        "loser's outbound block_links rows must be deleted, not orphaned"
    );

    // Idempotent: a second pass finds no duplicates and removes nothing.
    assert_eq!(
        provider.dedup_pages().await.expect("second dedup_pages"),
        0,
        "second pass must be a no-op"
    );
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    /// inv-page-name-unique: independent peers that each create the same-named
    /// page must converge on one page identity, so a merge yields no duplicate.
    ///
    /// Page identity is now a deterministic function of the normalized path
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
        } = holon_api::link_parser::classify_link(&name)
        {
            if scheme == "block" {
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
}

// ---------------------------------------------------------------------------
// Page-id collision after rename (EMPIRICAL probe, 2026-07-26)
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
// than step 1, and two distinct pages ("B" and "A") coexist.

/// Rename a page: an ordinary content edit on the existing entity (§5.3).
async fn rename_page(
    provider: &SqlOperationProvider,
    entity: &EntityName,
    id: &str,
    new_content: &str,
) {
    let mut p: holon_api::StorageEntity = HashMap::new();
    p.insert("id".into(), Value::String(id.to_string()));
    p.insert("content".into(), Value::String(new_content.to_string()));
    provider
        .execute_operation(entity, "update", p)
        .await
        .expect("rename page (update content)");
}

async fn page_rows(handle: &holon::storage::turso::DbHandle) -> Vec<(String, String)> {
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
async fn recreating_a_renamed_pages_old_name_yields_a_distinct_page() {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso");
    setup_schema(&handle).await;
    let provider = provider(handle.clone());
    let entity: EntityName = ENTITY.to_string().into();

    // 1. Create page A via the production lazy-create op.
    let mut op: holon_api::StorageEntity = HashMap::new();
    op.insert("target".into(), Value::String("A".to_string()));
    let id_a = match provider
        .execute_operation(&entity, "create_page_from_link", op)
        .await
        .expect("create page A")
        .response
    {
        Some(Value::String(s)) => s,
        other => panic!("expected leaf id, got {other:?}"),
    };

    // A child under A, so we can see whether it follows the renamed entity.
    create_page(&provider, &entity, "block:childOfA", "Child", &id_a).await;

    // 2. Rename A → B. Same entity, same id (§5.3).
    rename_page(&provider, &entity, &id_a, "B").await;
    assert_eq!(
        block_content(&handle, &id_a).await.as_deref(),
        Some("B"),
        "after rename the entity {id_a} must be titled B"
    );

    // 3. Create a NEW page A.
    let mut op2: holon_api::StorageEntity = HashMap::new();
    op2.insert("target".into(), Value::String("A".to_string()));
    let id_a2 = match provider
        .execute_operation(&entity, "create_page_from_link", op2)
        .await
        .expect("create page A the second time")
        .response
    {
        Some(Value::String(s)) => s,
        other => panic!("expected leaf id, got {other:?}"),
    };

    let pages = page_rows(&handle).await;

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
    let child_parent = block_parent(&handle, "block:childOfA")
        .await
        .expect("child must still have a parent");
    let child_parent_title = block_content(&handle, &child_parent).await;
    assert_eq!(
        child_parent_title.as_deref(),
        Some("B"),
        "the child must still hang under the RENAMED page B, but its parent \
         {child_parent} is titled {child_parent_title:?} — the new page A overwrote the \
         renamed entity instead of becoming a distinct one; observed pages: {pages:?}"
    );
}
