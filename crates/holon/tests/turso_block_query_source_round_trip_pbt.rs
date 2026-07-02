//! Round-trip PBT for the Turso [`BlockQuerySource`] (ADR 0004 Phase 9).
//!
//! Locks the equivalence the read seam promises: blocks written through the
//! production path and read back via `TursoBlockQuerySource::snapshot()` must
//! reproduce the generated blocks — field-for-field (id-keyed) **and** in
//! canonical sibling order.
//!
//! ```text
//! Vec<Block> --[execute_operation("block","create")]--> Turso (block matview)
//!            <--[TursoBlockQuerySource::snapshot()]----- Turso  (CDC LiveData mirror)
//! reference BlockSnapshot  ==  turso BlockSnapshot
//! ```
//!
//! Generators + the `NormalizedDocument` comparison come from
//! `holon-block-roundtrip-testing`, shared with the org / cache round-trip PBTs.

use holon::api::backend_engine::BackendEngine;
use holon::core::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_path;
use holon::storage::BLOCK_WRITE_TABLE;
use holon::storage::schema_module::SchemaModule;
use holon::sync::block_to_params;
use holon::sync::turso_block_query_source::TursoBlockQuerySource;
use holon_api::{EntityName, EntityUri, block::Block};
use holon_block_roundtrip_testing::{
    NormalizedDocument, assert_normalized_docs_equal, assert_sibling_order_matches, build_blocks,
    root_headlines_strategy,
};
use holon_core::OperationProvider;
use holon_core::storage::BlockQuerySource;
use holon_turso::schema_modules::BlockSchemaModule;
use proptest::prelude::*;
use tokio::runtime::Runtime;
use uuid::Uuid;

fn unique_db_path() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "holon_bqs_round_trip_{}_{}.db",
        std::process::id(),
        id
    ))
}

/// Write `blocks` through the production create path, assigning each a
/// document-order `sort_key` so canonical sibling order round-trips. `blocks`
/// arrives in pre-order from `build_blocks`, so the enumeration index is a
/// monotonic per-parent order key.
async fn write_blocks(engine: &BackendEngine, blocks: &[Block]) -> Result<(), TestCaseError> {
    // Write through the production block provider directly on the engine's
    // handle (the core test engine doesn't auto-register a "block" provider);
    // the matview projects incrementally, so the engine's `watch_view` sees the
    // rows. Edge fields fan `tags`/`requires` into the junction tables.
    let provider = SqlOperationProvider::with_edge_fields(
        engine.db_handle().clone(),
        BLOCK_WRITE_TABLE.to_string(),
        "block".to_string(),
        "block".to_string(),
        BlockSchemaModule.edge_fields(),
    );
    let entity: EntityName = "block".to_string().into();
    for (i, b) in blocks.iter().enumerate() {
        let params = block_to_params(&holon::api::SnapshotBlock {
            block: b.clone(),
            sort_key: format!("{i:010}"),
        });
        provider
            .execute_operation(&entity, "create", params)
            .await
            .map_err(|e| TestCaseError::fail(format!("create {}: {e}", b.id)))?;
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 20, ..ProptestConfig::default() })]

    /// Blocks round-trip through `TursoBlockQuerySource::snapshot()` —
    /// fields (id-keyed) and per-parent sibling order.
    #[test]
    fn block_round_trips_through_turso_query_source(headlines in root_headlines_strategy()) {
        let rt = Runtime::new().unwrap();
        let result: Result<(), TestCaseError> = rt.block_on(async {
            let engine = create_test_engine_with_path(unique_db_path())
                .await
                .map_err(|e| TestCaseError::fail(format!("engine init: {e}")))?;

            let doc_id = EntityUri::block(&Uuid::new_v4().to_string());
            let blocks = build_blocks(&doc_id, &headlines);

            write_blocks(&engine, &blocks).await?;

            let source = TursoBlockQuerySource::watch_default(&engine)
                .await
                .map_err(|e| TestCaseError::fail(format!("watch: {e}")))?;
            let snapshot = source
                .snapshot()
                .await
                .map_err(|e| TestCaseError::fail(format!("snapshot: {e}")))?;

            let expected = NormalizedDocument::from_blocks(None, &blocks);
            let actual = NormalizedDocument::from_block_snapshot(None, &snapshot);
            assert_normalized_docs_equal(&expected, &actual, "turso_block_query_round_trip")?;
            assert_sibling_order_matches(&blocks, &snapshot, "turso_block_query_round_trip")?;

            Ok(())
        });
        result?;
    }
}
