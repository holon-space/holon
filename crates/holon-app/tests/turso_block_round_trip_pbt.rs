//! Block round-trip PBT through the **Turso storage layer**.
//!
//! Sister test to `crates/holon-orgmode/tests/org_block_round_trip_pbt.rs`.
//! Both PBTs share generators + invariants from
//! `holon-block-roundtrip-testing`; each owns its own write/read pair.
//!
//! Direction tested here:
//! ```text
//! Vec<Block> --[block_to_params + SqlOperationProvider::execute_operation("create")]--> Turso
//!            <--[CacheBlockReader::get_blocks(doc_id)]------------------------------ Turso
//! assert_normalized_docs_equal(generated, parsed)   (full Block round-trip)
//! ```
//!
//! Two assertions in one PBT:
//!   1. `block_tags` junction must contain every (block_id, tag) pair the
//!      writer was asked to persist. Exercises the `SqlOperationProvider`
//!      edge-resolver fan-out.
//!   2. Round-tripping through `CacheBlockReader::get_blocks` must reproduce
//!      every generated `Block` field-for-field, *including* edge-typed fields
//!      like `tags`. **This is the regression gate for the production hydration
//!      bug** — the leak the SUT join in `pbt/sut.rs::live_block_tags`
//!      paper-overs. Today's `CacheBlockReader` only reads from
//!      `block.properties` (not from `block_tags`), so the assertion fails on
//!      `tags` mismatch. It will go green once Task #5 (CacheBlockReader
//!      hydration) ships, after which Task #1 (drop the SUT join) becomes safe.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;

use holon::core::queryable_cache::QueryableCache;
use holon::core::SqlOperationProvider;
use holon::storage::schema_module::SchemaModule;
use holon::storage::turso::TursoBackend;
use holon::storage::BLOCK_WRITE_TABLE;
use holon::sync::block_to_params;
use holon_api::block::Block;
use holon_api::EntityName;
use holon_app::turso_seams::CacheBlockReader;
use holon_block_roundtrip_testing::assert_normalized_docs_equal;
use holon_block_roundtrip_testing::build_blocks;
use holon_block_roundtrip_testing::root_headlines_strategy;
use holon_block_roundtrip_testing::NormalizedDocument;
use holon_core::OperationProvider;
use holon_filesystem::BlockReader;
use holon_turso::schema_modules::BlockSchemaModule;
use proptest::prelude::*;
use tokio::runtime::Runtime;
use uuid::Uuid;

async fn setup_production_schema(handle: &holon::storage::turso::DbHandle) {
    use holon::storage::schema_module::SchemaModule;
    use holon_turso::schema_modules::BlockMatviewSchemaModule;
    use holon_turso::schema_modules::CoreSchemaModule;

    handle
        .execute_ddl("PRAGMA foreign_keys = ON")
        .await
        .expect("FKs");
    // Match production: `block_raw` table + junction tables + `block` matview.
    CoreSchemaModule
        .ensure_schema(handle)
        .await
        .expect("CoreSchemaModule");
    BlockSchemaModule
        .ensure_schema(handle)
        .await
        .expect("BlockSchemaModule");
    BlockMatviewSchemaModule
        .ensure_schema(handle)
        .await
        .expect("BlockMatviewSchemaModule");
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 30,
        ..ProptestConfig::default()
    })]

    /// Lightweight invariant: every (block_id, tag) pair the writer is
    /// asked to persist must show up in the `block_tags` junction.
    /// Exercises the `SqlOperationProvider` edge-resolver fan-out only —
    /// passes today.
    #[test]
    fn turso_writer_fans_tags_into_junction(headlines in root_headlines_strategy()) {
        let rt = Runtime::new().unwrap();
        let result: Result<(), TestCaseError> = rt.block_on(async {
            let (_backend, handle) = TursoBackend::new_in_memory()
                .await
                .map_err(|e| TestCaseError::fail(format!("turso init: {e}")))?;
            setup_production_schema(&handle).await;

            let descriptors = BlockSchemaModule.edge_fields();
            let provider = Arc::new(SqlOperationProvider::with_edge_fields(
                handle.clone(),
                BLOCK_WRITE_TABLE.to_string(),
                "block".to_string(),
                "block".to_string(),
                descriptors,
            ));
            let entity: EntityName = "block".to_string().into();

            let doc_id = holon_api::EntityUri::block(&Uuid::new_v4().to_string());
            let blocks = build_blocks(&doc_id, &headlines);

            let mut expected_pairs: BTreeSet<(String, String)> = BTreeSet::new();
            for b in &blocks {
                for tag in b.tags.iter() {
                    expected_pairs.insert((b.id.to_string(), tag.clone()));
                }
            }

            for b in &blocks {
                let params = block_to_params(&holon::api::SnapshotBlock {
                    block: b.clone(),
                    sort_key: "A0".to_string(),
                });
                provider
                    .execute_operation(&entity, "create", params)
                    .await
                    .map_err(|e| TestCaseError::fail(format!("create {}: {e}", b.id)))?;
            }

            let rows = handle
                .query(
                    "SELECT block_id, tag FROM block_tags ORDER BY block_id, tag",
                    HashMap::new(),
                )
                .await
                .map_err(|e| TestCaseError::fail(format!("query block_tags: {e}")))?;
            let mut actual_pairs: BTreeSet<(String, String)> = BTreeSet::new();
            for r in rows {
                let bid = r
                    .get("block_id")
                    .and_then(|v| v.as_string())
                    .map(String::from)
                    .unwrap_or_default();
                let tag = r
                    .get("tag")
                    .and_then(|v| v.as_string())
                    .map(String::from)
                    .unwrap_or_default();
                actual_pairs.insert((bid, tag));
            }

            prop_assert_eq!(
                expected_pairs,
                actual_pairs,
                "block_tags junction must contain every (block_id, tag) pair the writer was asked to persist"
            );

            Ok(())
        });
        result?;
    }

    /// Full Block round-trip via `CacheBlockReader::get_blocks(doc_id)`.
    #[test]
    fn block_round_trips_through_cache_block_reader(headlines in root_headlines_strategy()) {
        let rt = Runtime::new().unwrap();
        let result: Result<(), TestCaseError> = rt.block_on(async {
            let (_backend, handle) = TursoBackend::new_in_memory()
                .await
                .map_err(|e| TestCaseError::fail(format!("turso init: {e}")))?;
            setup_production_schema(&handle).await;

            let descriptors = BlockSchemaModule.edge_fields();
            let provider = Arc::new(SqlOperationProvider::with_edge_fields(
                handle.clone(),
                BLOCK_WRITE_TABLE.to_string(),
                "block".to_string(),
                "block".to_string(),
                descriptors,
            ));
            let entity: EntityName = "block".to_string().into();

            let doc_id = holon_api::EntityUri::block(&Uuid::new_v4().to_string());
            let blocks = build_blocks(&doc_id, &headlines);

            // Production write path with edge resolver fans tags into
            // `block_tags` and requires into `block_requires`.
            for b in &blocks {
                let params = block_to_params(&holon::api::SnapshotBlock {
                    block: b.clone(),
                    sort_key: "A0".to_string(),
                });
                provider
                    .execute_operation(&entity, "create", params)
                    .await
                    .map_err(|e| TestCaseError::fail(format!("create {}: {e}", b.id)))?;
            }

            // Production read path: CacheBlockReader::get_blocks.
            // CREATE TABLE IF NOT EXISTS in QueryableCache::new is a no-op
            // here because setup_production_schema already created it.
            let cache: Arc<QueryableCache<Block>> = Arc::new(
                QueryableCache::<Block>::new(handle.clone(), Block::type_definition())
                    .await
                    .map_err(|e| TestCaseError::fail(format!("create cache: {e}")))?,
            );
            let block_reader: Arc<dyn BlockReader> =
                Arc::new(CacheBlockReader::new(cache));

            let actual_blocks = block_reader
                .get_blocks(&doc_id)
                .await
                .map_err(|e| TestCaseError::fail(format!("get_blocks: {e}")))?;

            let expected = NormalizedDocument::from_blocks(None, &blocks);
            let actual = NormalizedDocument::from_blocks(None, &actual_blocks);

            assert_normalized_docs_equal(&expected, &actual, "turso_round_trip")?;

            Ok(())
        });
        result?;
    }
}

/// Directed store round-trip for the blocked-by / `requires` dependency edge.
///
/// `:BLOCKED-BY:` is the org-drawer alias of the `requires` edge field (single
/// `block_requires` junction; owner ruling 2026-07-16 — see ORG_SYNTAX.md).
/// This exercises that edge end-to-end through the SQL provider's generic edge
/// partition: create → `block_requires` junction rows → `CacheBlockReader`
/// hydration back into `Block.requires`.
///
/// It is a directed test (not folded into the proptest above) because the
/// proptest harness is independently RED on main — it parents generated root
/// headlines to a `doc_id` it never inserts, violating the `block_raw` parent
/// FK. This test seeds the document block so every parent FK is satisfied.
#[test]
fn blocked_by_requires_edge_round_trips_through_store() {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let (_backend, handle) = TursoBackend::new_in_memory().await.expect("turso init");
        setup_production_schema(&handle).await;

        let descriptors = BlockSchemaModule.edge_fields();
        let provider = Arc::new(SqlOperationProvider::with_edge_fields(
            handle.clone(),
            BLOCK_WRITE_TABLE.to_string(),
            "block".to_string(),
            "block".to_string(),
            descriptors,
        ));
        let entity: EntityName = "block".to_string().into();

        // Seed the document root (parent = sentinel:no_parent, seeded by
        // CoreSchemaModule) so the blocks beneath it satisfy the parent FK.
        let doc_id = holon_api::EntityUri::block(&Uuid::new_v4().to_string());
        let target_id = holon_api::EntityUri::block("dep-target-a");
        let dependent_id = holon_api::EntityUri::block("dependent-b");

        let doc = Block::new_text(doc_id.clone(), holon_api::EntityUri::no_parent(), "Doc");
        let target = Block::new_text(target_id.clone(), doc_id.clone(), "A");
        let mut dependent = Block::new_text(dependent_id.clone(), doc_id.clone(), "B");
        // `dependent` is blocked-by `target` (the `requires` edge).
        dependent.requires = vec![target_id.clone()];

        for blk in [doc, target, dependent.clone()] {
            let params = block_to_params(&holon::api::SnapshotBlock {
                block: blk.clone(),
                sort_key: "A0".to_string(),
            });
            provider
                .execute_operation(&entity, "create", params)
                .await
                .unwrap_or_else(|e| panic!("create {}: {e}", blk.id));
        }

        // (1) The edge was partitioned into the block_requires junction.
        let junction = handle
            .query(
                "SELECT block_id, required_id FROM block_requires ORDER BY block_id",
                HashMap::new(),
            )
            .await
            .expect("query block_requires");
        let pairs: Vec<(String, String)> = junction
            .iter()
            .map(|r| {
                (
                    r.get("block_id")
                        .and_then(|v| v.as_string())
                        .unwrap()
                        .to_string(),
                    r.get("required_id")
                        .and_then(|v| v.as_string())
                        .unwrap()
                        .to_string(),
                )
            })
            .collect();
        assert_eq!(
            pairs,
            vec![(dependent_id.to_string(), target_id.to_string())],
            "the blocked-by/requires edge must land as exactly one block_requires row"
        );

        // (2) CacheBlockReader hydration reconstructs Block.requires from the
        //     junction (the production read path).
        let cache: Arc<QueryableCache<Block>> = Arc::new(
            QueryableCache::<Block>::new(handle.clone(), Block::type_definition())
                .await
                .expect("cache"),
        );
        let reader: Arc<dyn BlockReader> = Arc::new(CacheBlockReader::new(cache));
        let blocks = reader.get_blocks(&doc_id).await.expect("get_blocks");
        let got = blocks
            .iter()
            .find(|b| b.id == dependent_id)
            .expect("dependent block must hydrate");
        assert_eq!(
            got.requires,
            vec![target_id.clone()],
            "hydrated Block.requires must reproduce the blocked-by edge"
        );
    });
}
