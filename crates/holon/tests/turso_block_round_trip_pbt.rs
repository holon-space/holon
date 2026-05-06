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
//!      every generated `Block` field-for-field, *including* edge-typed
//!      fields like `tags`. **This is the regression gate for the
//!      production hydration bug** — the leak the SUT join in
//!      `pbt/sut.rs::live_block_tags` paper-overs. Today's
//!      `CacheBlockReader` only reads from `block.properties` (not from
//!      `block_tags`), so the assertion fails on `tags` mismatch. It will
//!      go green once Task #5 (CacheBlockReader hydration) ships, after
//!      which Task #1 (drop the SUT join) becomes safe.

use holon::core::SqlOperationProvider;
use holon::core::datasource::OperationProvider;
use holon::core::queryable_cache::QueryableCache;
use holon::storage::BLOCK_WRITE_TABLE;
use holon::storage::schema_module::SchemaModule;
use holon::storage::schema_modules::BlockSchemaModule;
use holon::storage::turso::TursoBackend;
use holon::sync::block_to_params;
use holon_api::{EntityName, block::Block};
use holon_block_roundtrip_testing::{
    NormalizedDocument, assert_normalized_docs_equal, build_blocks, root_headlines_strategy,
};
use holon_orgmode::di::CacheBlockReader;
use holon_orgmode::traits::BlockReader;
use proptest::prelude::*;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use tokio::runtime::Runtime;
use uuid::Uuid;

async fn setup_production_schema(handle: &holon::storage::turso::DbHandle) {
    use holon::storage::schema_module::SchemaModule;
    use holon::storage::schema_modules::{BlockMatviewSchemaModule, CoreSchemaModule};

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
                let params = block_to_params(b);
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
    /// **Currently red on tag mismatch** — the production hydration bug
    /// that this PBT exists to gate. Will go green after Task #5
    /// (CacheBlockReader hydration) lands.
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
                let params = block_to_params(b);
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
