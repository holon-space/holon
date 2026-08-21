//! Stage-1 LogSeq-DB import, increment 4: the blocks land in a REAL store.
//!
//! The in-crate tests of `holon-logseq-db` prove what the importer HANDS
//! the store — the create batch and the stated sibling order. This proves what
//! the store then DOES with it, which is the only place three things can be
//! checked at all:
//!
//!  1. all 206 uuid-bearing entities exist as rows, addressed by their LogSeq
//!     uuid — identity survives the store, not just the projection;
//!  2. the RE-MINTED sibling order matches LogSeq's fracdex sequence. The
//!     importer never stores LogSeq's `:block/order` (invariants 2/3/10 — the
//!     order owner mints keys), so nothing before this point can tell whether
//!     the minted `sort_key`s preserved the sequence or quietly permuted it;
//!  3. amendment A5: `:block/refs` is dropped on import on the premise that
//!     Holon RE-DERIVES the reference graph. Only a real store has the
//!     `block_links` junction and `backlinks` matview that premise names.
//!
//! Assertion 3 is expected RED on arrival, for a real reason rather than a
//! missing symbol: `block_links` is derived from a block's inline MARKS
//! (`holon_api::derive_block_links`), `marks` is a STORED field the writer
//! supplies, and the projection supplies none — so the import currently loses
//! the entire reference graph while every count stays green. This is the
//! tripwire for that.
//!
//! @pbt kind harness
//! @pbt covers logseq-db-import — read-only LogSeq DB-graph import (stage 1)

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use holon_api::BlockContent;
use holon_api::BlockEdges;
use holon_api::EntityUri;
use holon_api::Value;
use holon_core::block_ordering::BlockCreateRequest;
use holon_core::block_ordering::BlockOrdering;
use holon_integration_tests::TestEnvironmentBuilder;
use holon_logseq_db::ingest::enter_store;
use holon_logseq_db::project;
use holon_logseq_db::read_datoms;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

/// The committed fixture lives with the importer crate that owns it.
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../holon-logseq-db/tests/fixtures/logseq-db/holontest.sqlite")
}

const EXPECTED_BLOCKS: usize = 206;

/// The Aug-20 journal and its children in fracdex order — measured from the
/// fixture with the stage-0 spike's Python decoder, independently of the Rust.
const AUG20_UUID: &str = "00000001-2026-0820-0000-000000000000";
const AUG20_CHILDREN_IN_FRACDEX_ORDER: &[&str] = &[
    "6a86c98a-d818-4787-8ff1-e3b619b15f2d", // a0
    "6a86d4a0-4ae2-4ecd-98bf-c5a10e36604a", // a01
    "6a86d453-ebf9-4a43-a4dc-6a29f19fee38", // a02
    "6a86d3d4-82f8-4bb7-becc-a06ffb3e814e", // a04
    "6a86cfb1-07a4-4c07-9c7b-477438c99fad", // a08
    "6a86cf5f-2cc4-4f32-b6ba-9496235db709", // a0G
    "6a86ce83-7433-4313-b8a8-57295bb08feb", // a0V
    "6a86cdb3-e5af-4a4b-8bca-4100966474b1", // a1
    "6a86ce9d-9fe6-434e-b07f-bd629bb68ae9", // a2
];

/// Project Alpha, and the ONE block that references it: e206, whose title is
/// `Link to [[6a86cf74-…]]`. LogSeq DB carries links inside the title text, so
/// the reference graph is recoverable from content the import already keeps.
const PROJECT_ALPHA_UUID: &str = "6a86cf74-3882-4ebd-a19d-c1fa46f58380";
const LINKING_BLOCK_UUID: &str = "6a86cf5f-2cc4-4f32-b6ba-9496235db709";

/// One `execute_query` result row, as the other integration tests spell it.
type Row = HashMap<Arc<str>, Value>;

async fn query(env: &holon_integration_tests::TestEnvironment, sql: &str) -> Vec<Row> {
    env.engine()
        .execute_query(sql.to_string(), HashMap::new(), None)
        .await
        .unwrap_or_else(|e| panic!("query {sql:?}: {e:#}"))
}

fn string_field(row: &Row, column: &str) -> String {
    match row.get(column) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => format!("{other:?}"),
    }
}

/// Minimal probe of the ONE `BlockOrdering` contract the import leans on:
/// `create_in_tree_batch` then `place_all` yields the stated sibling order.
///
/// Deliberately independent of the importer — three hand-made blocks, created
/// in one order and re-stated in the reverse. If this fails, the import's
/// order handling is not the suspect; the ordering authority is.
#[test]
fn place_all_restates_sibling_order_after_a_create_batch() {
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .build(rt.clone())
            .await
            .expect("boot an empty vault");
        let ordering = env
            .injector()
            .expect("start_app must capture the injector")
            .resolve_async::<dyn BlockOrdering>()
            .await;

        let parent = EntityUri::block("11111111-0000-4000-8000-00000000aaaa");
        let children: Vec<EntityUri> = ["bbbb", "cccc", "dddd"]
            .iter()
            .map(|tag| EntityUri::block(&format!("11111111-0000-4000-8000-00000000{tag}")))
            .collect();

        let mut requests = vec![BlockCreateRequest {
            parent_id: EntityUri::no_parent(),
            id: parent.clone(),
            content: BlockContent::Text {
                raw: "parent".to_string(),
            },
            properties: HashMap::new(),
            edges: BlockEdges::default(),
        }];
        for (i, child) in children.iter().enumerate() {
            requests.push(BlockCreateRequest {
                parent_id: parent.clone(),
                id: child.clone(),
                content: BlockContent::Text {
                    raw: format!("child {i}"),
                },
                properties: HashMap::new(),
                edges: BlockEdges::default(),
            });
        }
        let persisted = ordering
            .create_in_tree_batch(&requests)
            .await
            .expect("create the probe blocks");
        // Container-specific: the default harness is Turso-backed, whose
        // ordering authority accepts `create_in_tree`. In a LoroMemory
        // environment the authority routes creates through `update_in_tree`
        // instead and returns `false` here — that is a store-mode difference,
        // not a defect, so this assertion would fail benignly there.
        assert!(
            persisted.iter().all(|p| *p),
            "the authority declined a probe create: {persisted:?}"
        );

        let as_created = ordering.children(&parent).await.expect("children");
        assert_eq!(as_created, children, "creation order is request order");

        // Now state the REVERSE order and read it back.
        let reversed: Vec<EntityUri> = children.iter().rev().cloned().collect();
        ordering
            .place_all(&parent, &reversed)
            .await
            .expect("place_all the reversed order");
        env.wait_for_loro_quiescence(Duration::from_secs(30)).await;

        let after_place = ordering.children(&parent).await.expect("children");
        assert_eq!(
            after_place, reversed,
            "place_all must restate the total sibling order in the authority"
        );
    });
}

#[test]
fn logseq_db_graph_imports_into_a_real_store() {
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .build(rt.clone())
            .await
            .expect("boot an empty vault for the import");

        // --- import: decode → project → enter the store ---
        let set = read_datoms(&fixture_path())
            .await
            .expect("read the HolonTest datom set");
        let projection = project(&set).expect("project the datoms into blocks");
        let ordering = env
            .injector()
            .expect("start_app must capture the injector")
            .resolve_async::<dyn BlockOrdering>()
            .await;
        let report = enter_store(&projection, ordering.as_ref())
            .await
            .expect("enter the store through BlockOrdering");
        assert_eq!(report.blocks_created, EXPECTED_BLOCKS);
        // A decline is not a failure at the call site, so it has to be asserted
        // here: an authority that routes the creates elsewhere cannot own their
        // order either, and the import would look successful anyway. Same
        // container-specificity as the probe above — Turso-backed accepts
        // `create_in_tree`; a LoroMemory environment would decline benignly.
        assert_eq!(
            report.blocks_persisted_by_authority, EXPECTED_BLOCKS,
            "the ordering authority declined some creates; it cannot own the \
             sibling order of blocks it did not persist"
        );

        // Loro is the authority. Importing 206 blocks and then re-stating
        // every sibling order is a long burst of ops, so wait on the real
        // barriers rather than a guessed sleep — a fixed sleep here reads as
        // "the order was not applied" when it only means "not yet projected".
        env.wait_for_loro_quiescence(Duration::from_secs(60)).await;
        env.wait_for_cdc_quiescent(Duration::from_millis(500), Duration::from_secs(60))
            .await;

        // --- 1. every uuid-bearing entity exists as a row ---
        let rows = query(
            &env,
            "SELECT id, parent_id, sort_key, content FROM block_raw",
        )
        .await;
        let by_bare_id: HashMap<String, &Row> = rows
            .iter()
            .filter_map(|row| {
                let id = string_field(row, "id");
                Some((id.strip_prefix("block:")?.to_string(), row))
            })
            .collect();
        let mut missing: Vec<&str> = AUG20_CHILDREN_IN_FRACDEX_ORDER
            .iter()
            .copied()
            .chain([AUG20_UUID, PROJECT_ALPHA_UUID, LINKING_BLOCK_UUID])
            .filter(|uuid| !by_bare_id.contains_key(*uuid))
            .collect();
        missing.sort_unstable();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "imported blocks missing from the store: {missing:?} \
             ({} block rows present)",
            by_bare_id.len()
        );

        // --- 2a. the AUTHORITY's own tree order ---
        // Asked before the SQL projection so a wrong order localizes to one
        // side: authority wrong (place_all never took) vs projection wrong
        // (right in the tree, not carried into sort_key).
        let aug20 = EntityUri::block(AUG20_UUID);
        let authority_order: Vec<String> = ordering
            .children(&aug20)
            .await
            .expect("read the Aug-20 journal's children from the ordering authority")
            .iter()
            .map(|uri| {
                uri.as_str()
                    .strip_prefix("block:")
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            authority_order,
            AUG20_CHILDREN_IN_FRACDEX_ORDER
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
            "the ordering authority's tree must hold the fracdex sibling order \
             after place_all"
        );

        // --- 2. the re-minted order preserves the fracdex sequence ---
        // Read the children back BY SORT KEY — the store's own ordering — and
        // compare against the sequence LogSeq's fracdex implied.
        let aug20_uri = EntityUri::block(AUG20_UUID);
        let mut children: Vec<(String, String)> = rows
            .iter()
            .filter(|row| string_field(row, "parent_id") == aug20_uri.to_string())
            .map(|row| {
                (
                    string_field(row, "sort_key"),
                    string_field(row, "id")
                        .strip_prefix("block:")
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect();
        children.sort();
        let by_sort_key: Vec<String> = children.into_iter().map(|(_, id)| id).collect();
        assert_eq!(
            by_sort_key,
            AUG20_CHILDREN_IN_FRACDEX_ORDER
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
            "re-minted sort keys must reproduce LogSeq's fracdex sibling order"
        );

        // --- 3. amendment A5: the reference graph is re-derived ---
        let backlinks = query(
            &env,
            &format!("SELECT id FROM backlinks WHERE target_id = 'block:{PROJECT_ALPHA_UUID}'"),
        )
        .await;
        assert!(
            !backlinks.is_empty(),
            "A5: Project Alpha's backlinks are EMPTY — `:block/refs` was dropped on import on \
             the premise that Holon re-derives the reference graph, but block {LINKING_BLOCK_UUID} \
             (title `Link to [[{PROJECT_ALPHA_UUID}]]`) produced no link. The reference graph is \
             silently lost while every count stays green."
        );
    });
}
