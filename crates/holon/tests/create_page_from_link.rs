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
// So a page's id is a function of its path and of the pages already holding
// the ids that path derives: `PageId::for_path(path)`, stepping along
// `PageId::for_path_beside` past every page that holds an id but is not this
// page (docs/Plans/PageIdentityDeterminism.md §5.3). `expected_slot` states
// that model over the two id primitives; the properties hold `page_slot` and
// the real `create_page_from_link` writer to it.

/// 1–3 non-empty page-path segments and a `/` separator that may carry
/// surrounding spaces (`"Areas / Sub"`), the shape where the parser must trim
/// segments to agree with the writer.
fn page_path_parts_strategy() -> impl Strategy<Value = (Vec<String>, String)> {
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
}

/// A well-formed page path.
fn page_name_strategy() -> impl Strategy<Value = String> {
    page_path_parts_strategy().prop_map(|(segments, sep)| segments.join(&sep))
}

/// The trimmed segments of a page path and the `/`-joined path of each prefix.
fn segment_paths(name: &str) -> Vec<(String, String)> {
    let mut path = String::new();
    name.split('/')
        .map(|seg| {
            let seg = seg.trim().to_string();
            path = if path.is_empty() {
                seg.clone()
            } else {
                format!("{path}/{seg}")
            };
            (seg, path.clone())
        })
        .collect()
}

/// What holds one id along a page path's id chain.
#[derive(Debug, Clone, Copy)]
enum Holder {
    /// A page of another title (a rename kept the id).
    Renamed,
    /// A page of this title under another parent (a move kept the id).
    Moved,
    /// A page whose title differs from this one only in case or spacing,
    /// under another parent.
    MovedVariant,
    /// An untitled placeholder under another parent.
    MovedPlaceholder,
    /// A page whose title differs from this one only in case or spacing: the
    /// same page (PageIdentityDeterminism.md §5.3).
    Variant,
    /// An untitled placeholder for this id under this parent.
    Placeholder,
    /// This very page: this title under this parent.
    Same,
}

fn holder_strategy() -> impl Strategy<Value = Holder> {
    prop_oneof![
        3 => Just(Holder::Renamed),
        2 => Just(Holder::Moved),
        2 => Just(Holder::MovedVariant),
        1 => Just(Holder::MovedPlaceholder),
        2 => Just(Holder::Variant),
        1 => Just(Holder::Placeholder),
        1 => Just(Holder::Same),
    ]
}

/// The id chain `path` derives: `for_path`, then each `for_path_beside` step.
fn id_chain(path: &str, len: usize) -> Vec<holon_api::link_parser::PageId> {
    let mut ids = vec![holon_api::link_parser::PageId::for_path(path).unwrap()];
    while ids.len() < len {
        let next = holon_api::link_parser::PageId::for_path_beside(
            path,
            ids.last().unwrap().as_entity_uri(),
        )
        .unwrap();
        ids.push(next);
    }
    ids
}

/// The slot the model assigns when `holders[i]` holds the i-th id of the chain.
fn expected_slot(path: &str, holders: &[Holder]) -> holon_api::PageSlot {
    let ids = id_chain(path, holders.len() + 1);
    for (holder, id) in holders.iter().zip(&ids) {
        match holder {
            Holder::Renamed | Holder::Moved | Holder::MovedVariant | Holder::MovedPlaceholder => {}
            Holder::Placeholder => return holon_api::PageSlot::Create(id.clone()),
            Holder::Same | Holder::Variant => return holon_api::PageSlot::Existing(id.clone()),
        }
    }
    holon_api::PageSlot::Create(ids[holders.len()].clone())
}

fn variant_of(title: &str) -> String {
    let upper = title.to_uppercase();
    if upper != title {
        upper
    } else {
        format!("{title} ").replacen(' ', "  ", 1)
    }
}

/// The title and parent a `holder` of the page `title` under `parent` carries;
/// `elsewhere` is another parent.
fn holder_row(
    holder: Holder,
    title: &str,
    parent: &holon_api::EntityUri,
    elsewhere: &holon_api::EntityUri,
    renamed_title: String,
) -> holon_api::PageHolder {
    let (title, parent) = match holder {
        Holder::Renamed => (renamed_title, parent.clone()),
        Holder::Moved => (title.to_string(), elsewhere.clone()),
        Holder::MovedVariant => (variant_of(title), elsewhere.clone()),
        Holder::MovedPlaceholder => (String::new(), elsewhere.clone()),
        Holder::Variant => (variant_of(title), parent.clone()),
        Holder::Placeholder => (String::new(), parent.clone()),
        Holder::Same => (title.to_string(), parent.clone()),
    };
    holon_api::PageHolder { title, parent }
}

proptest! {
    /// `page_slot` binds a page to a holder under its parent whose title equals
    /// its own up to case and spacing, completes an untitled one there, and
    /// otherwise walks the beside chain as the model says.
    #[test]
    fn page_slot_follows_the_id_chain_model(
        name in page_name_strategy(),
        holders in proptest::collection::vec(holder_strategy(), 0..5),
    ) {
        let (title, path) = segment_paths(&name).pop().unwrap();
        let parent = holon_api::EntityUri::block("parent-of-the-leaf");
        let elsewhere = holon_api::EntityUri::block("moved-to");
        let held: HashMap<holon_api::EntityUri, holon_api::PageHolder> = holders
            .iter()
            .zip(id_chain(&path, holders.len()))
            .map(|(holder, id)| {
                let row = holder_row(*holder, &title, &parent, &elsewhere, "held-elsewhere".into());
                (id.into_entity_uri(), row)
            })
            .collect();
        let slot = futures::executor::block_on(holon_api::page_slot(&path, &parent, &title, |id| {
            std::future::ready(Ok(held.get(&id).cloned()))
        }))
        .expect("page_slot");
        prop_assert_eq!(slot, expected_slot(&path, &holders), "holders {:?}", holders);
    }

    /// inv-page-name-unique: the id an empty store gives a page is its path id,
    /// so independent peers creating the same-named page converge; and the link
    /// parser's optimistic id for the target equals it.
    #[test]
    fn inv_page_name_unique_converges_across_peers(name in page_name_strategy()) {
        let (_, path) = segment_paths(&name).pop().unwrap();
        let id = expected_slot(&path, &[]);
        let path_id = holon_api::link_parser::PageId::for_path(&name).unwrap();
        prop_assert_eq!(&id, &holon_api::PageSlot::Create(path_id.clone()));
        if let holon_api::link_parser::LinkTarget::CreationIntent { scheme, target_id, .. } =
            holon_api::link_parser::LinkTargetClassifier::default().classify(&name)
            && scheme == "block"
        {
            prop_assert_eq!(
                target_id.as_str(),
                path_id.as_str(),
                "parser/writer page-id divergence for target {:?}",
                name
            );
        }
    }
}

/// The cases share one engine, so each case's segments carry the case number:
/// no case finds another case's pages by name.
const WRITER_CASES: u32 = 32;

/// Distinct segments up to case and spacing: a repeated one resolves to its
/// ancestor by name.
fn writer_path_strategy() -> impl Strategy<Value = (Vec<String>, String)> {
    page_path_parts_strategy().prop_filter(
        "a repeated segment resolves to its ancestor by name",
        |(segments, _)| {
            segments
                .iter()
                .map(|s| holon_api::PageTitleKey::of(s))
                .collect::<std::collections::HashSet<_>>()
                .len()
                == segments.len()
        },
    )
}

/// The holders of one segment's id chain, as the writer meets them: pages it
/// must pass, then at most one it binds to. A page of this exact title under
/// another parent is left out: the writer reaches it by name
/// (`resolve_page_name`), before any id is derived.
fn writer_chain_strategy() -> impl Strategy<Value = Vec<Holder>> {
    let pass = prop_oneof![
        Just(Holder::Renamed),
        Just(Holder::MovedVariant),
        Just(Holder::MovedPlaceholder),
    ];
    let bind = proptest::option::of(prop_oneof![
        Just(Holder::Same),
        Just(Holder::Variant),
        Just(Holder::Placeholder),
    ]);
    (proptest::collection::vec(pass, 0..3), bind).prop_map(|(mut chain, bind)| {
        chain.extend(bind);
        chain
    })
}

/// The real writer mints or binds every page of a generated path at the
/// model's id: it passes pages that hold the derived ids elsewhere or under
/// another title, completes an untitled placeholder at this position, binds a
/// page of this title up to case and spacing, and leaves every other holder
/// untouched.
#[test]
fn create_page_from_link_mints_the_model_ids() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let engine = rt.block_on(block_engine());
    let case = std::sync::atomic::AtomicUsize::new(0);
    let strategy = (
        writer_path_strategy(),
        proptest::collection::vec(writer_chain_strategy(), 3),
    );
    proptest::test_runner::TestRunner::new(ProptestConfig::with_cases(WRITER_CASES))
        .run(&strategy, |((segments, sep), chains)| {
            let n = case.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let name = segments
                .iter()
                .map(|s| format!("{s} c{n}"))
                .collect::<Vec<_>>()
                .join(&sep);
            rt.block_on(writer_mints_the_model_ids(&engine, n, &name, &chains));
            Ok(())
        })
        .unwrap_or_else(|e| panic!("{e}"));
}

/// For each segment i in turn: occupy its id chain with `chains[i]` (under the
/// page segment i-1 resolved to, which must exist first), then click a link to
/// the path up to segment i; the last click is a link to `name` itself.
async fn writer_mints_the_model_ids(
    engine: &BackendEngine,
    case: usize,
    name: &str,
    chains: &[Vec<Holder>],
) {
    let handle = engine.db_handle();
    let elsewhere = holon_api::EntityUri::block(&format!("elsewhere-c{case}"));
    create_page(
        engine,
        elsewhere.as_str(),
        &format!("elsewhere-c{case}"),
        "sentinel:no_parent",
    )
    .await;
    let segments = segment_paths(name);
    let mut parent = holon_api::EntityUri::no_parent();
    let mut untouched = Vec::new();
    let mut expected = Vec::new();
    for (i, ((title, path), chain)) in segments.iter().zip(chains).enumerate() {
        let ids = id_chain(path, chain.len());
        for (j, (holder, id)) in chain.iter().zip(&ids).enumerate() {
            let row = holder_row(
                *holder,
                title,
                &parent,
                &elsewhere,
                format!("held-{path}-{j}"),
            );
            create_page(engine, id.as_str(), &row.title, row.parent.as_str()).await;
            if !matches!(holder, Holder::Placeholder | Holder::Same | Holder::Variant) {
                untouched.push((id.as_str().to_string(), row));
            }
        }
        let id = match expected_slot(path, chain) {
            holon_api::PageSlot::Create(id) | holon_api::PageSlot::Existing(id) => id,
        };
        let stored_title = match chain.last() {
            Some(Holder::Variant) => variant_of(title),
            _ => title.clone(),
        };
        expected.push((id.as_str().to_string(), stored_title, parent.clone()));
        let target = if i + 1 == segments.len() {
            name
        } else {
            path.as_str()
        };
        let leaf = create_page_from_link(engine, target)
            .await
            .expect("create_page_from_link");
        assert_eq!(leaf, id.as_str(), "leaf of {target:?}, holders {chains:?}");
        parent = id.into_entity_uri();
    }

    for (id, title, parent) in &expected {
        assert_eq!(
            block_content(handle, id).await.as_deref(),
            Some(title.as_str()),
            "{name:?}, holders {chains:?}: page {id}"
        );
        assert_eq!(
            block_parent(handle, id).await.as_deref(),
            Some(parent.as_str()),
            "{name:?}, holders {chains:?}: parent of {id}"
        );
    }
    for (id, row) in &untouched {
        assert_eq!(
            (
                block_content(handle, id).await.unwrap_or_default(),
                block_parent(handle, id).await.as_deref()
            ),
            (row.title.clone(), Some(row.parent.as_str())),
            "{name:?}, holders {chains:?}: the page holding {id} was changed"
        );
    }
}

/// Two fresh stores that each click the same link mint the same page ids.
#[test]
fn two_fresh_engines_mint_the_same_page_chain_ids() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let peers = [rt.block_on(block_engine()), rt.block_on(block_engine())];
    let case = std::sync::atomic::AtomicUsize::new(0);
    proptest::test_runner::TestRunner::new(ProptestConfig::with_cases(8))
        .run(&writer_path_strategy(), |(segments, sep)| {
            let n = case.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let name = segments
                .iter()
                .map(|s| format!("{s} p{n}"))
                .collect::<Vec<_>>()
                .join(&sep);
            let chains = rt.block_on(async {
                let mut chains = Vec::new();
                for peer in &peers {
                    create_page_from_link(peer, &name)
                        .await
                        .expect("create_page_from_link");
                    let mut chain = Vec::new();
                    for (_, path) in segment_paths(&name) {
                        let id = holon_api::link_parser::PageId::for_path(&path).unwrap();
                        chain.push((
                            id.as_str().to_string(),
                            block_parent(peer.db_handle(), id.as_str()).await,
                        ));
                    }
                    chains.push(chain);
                }
                chains
            });
            prop_assert_eq!(&chains[0], &chains[1], "{:?}", name);
            prop_assert!(
                chains[0].iter().all(|(_, parent)| parent.is_some()),
                "{:?}: a path id holds no page: {:?}",
                name,
                chains[0]
            );
            Ok(())
        })
        .unwrap_or_else(|e| panic!("{e}"));
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
// than step 1, and two distinct pages ("B" and "A") coexist.

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
    let id_a2 = create_page_from_link(&engine, "A")
        .await
        .expect("recreating page A must succeed (§5.3)");

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

/// Create a block `source` whose content links `[[A]]` by name.
async fn link_to_a(engine: &BackendEngine, source: &str) {
    let mut p = create_params(source, "see A");
    p.insert("marks".into(), Value::String(name_link_marks("A", 4, 5)));
    fixture_op(engine, "create", p).await;
}

/// A `[[A]]` name link written after page A was renamed resolves to the page
/// NAMED `A`, never to the renamed page that still holds
/// `PageId::for_path("A")`.
#[tokio::test(flavor = "multi_thread")]
async fn a_name_link_resolves_to_the_page_with_that_name_after_its_old_holder_was_renamed() {
    let engine = block_engine().await;
    let handle = engine.db_handle();

    let id_a = create_page_from_link(&engine, "A")
        .await
        .expect("create page A");
    rename_page(&engine, &id_a, "B").await;
    link_to_a(&engine, "before").await;
    assert_eq!(
        link_resolved(handle, "before").await,
        None,
        "with no page named A, a link to A must dangle, not resolve to the renamed page {id_a}"
    );

    let id_a2 = create_page_from_link(&engine, "A")
        .await
        .expect("create the new page A");
    assert_ne!(id_a2, id_a);
    link_to_a(&engine, "after").await;

    for source in ["before", "after"] {
        assert_eq!(
            link_resolved(handle, source).await.as_deref(),
            Some(id_a2.as_str()),
            "link [[A]] in block {source} must resolve to the page named A ({id_a2}), not to the \
             renamed page B ({id_a})"
        );
    }
}

/// A `[[A]]` link resolved while page A still had that name.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a rename does not re-resolve block_links rows; docs/Testing/bugfunnel/entries/2026-10-08-new-file-at-a-renamed-pages-old-path-takes-over-its-id.md"]
async fn a_link_resolved_before_a_rename_resolves_to_the_page_with_that_name() {
    let engine = block_engine().await;
    let handle = engine.db_handle();

    let id_a = create_page_from_link(&engine, "A")
        .await
        .expect("create page A");
    link_to_a(&engine, "early").await;
    assert_eq!(
        link_resolved(handle, "early").await.as_deref(),
        Some(id_a.as_str())
    );
    rename_page(&engine, &id_a, "B").await;
    let id_a2 = create_page_from_link(&engine, "A")
        .await
        .expect("create the new page A");

    assert_eq!(
        link_resolved(handle, "early").await.as_deref(),
        Some(id_a2.as_str()),
        "link [[A]] must resolve to the page named A ({id_a2}), not to the renamed page B ({id_a})"
    );
}
