//! The `emits ⊇ affects` consistency lock (ADR 0031 Increment 2, OQ-2 = A).
//!
//! `#[affects]` and `#[emits]` both say what an op writes, for different
//! consumers (pie-menu auto-attachment vs. simulation). Two such declarations
//! drifting apart is the parallel-catalog failure the unification guard names
//! as a kill criterion. The ruled resolution keeps both and locks them
//! together here; deriving `affected_fields` from `emits` and deleting
//! `#[affects]` stays the convergence target.
//!
//! The mapping is `affects("f") ↦ block.f`: every `#[affects]` string in the
//! tree is a bare block column name. Should a UI-synthetic field ever appear
//! there, this lock fails loudly and the carve-out gets written down rather
//! than discovered later as drift.
//!
//! Undeclared ops are skipped by construction — they make no claim to check.

use std::sync::Arc;

use holon::api::BackendEngine;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::TransitionArcs;
use holon_api::block::Block;
use holon_core::OperationProvider;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

const BLOCK: &str = "block";

/// The same production SqlOnly block wiring the boundary-behavior lock uses, so
/// both oracles read the SAME catalog a live block's profile carries.
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
    .expect("test engine with block providers")
}

#[tokio::test(flavor = "multi_thread")]
async fn every_declared_ops_emits_covers_its_affects() {
    let engine = block_engine().await;
    let catalog = engine.available_operations(BLOCK).await;

    assert!(
        !catalog.is_empty(),
        "block catalog must be non-empty — a vacuous catalog would let this lock \
         pass trivially"
    );

    let mut checked = 0usize;
    for descriptor in &catalog {
        let TransitionArcs::Declared { .. } = descriptor.arcs else {
            continue;
        };
        let written: Vec<String> = descriptor
            .arcs
            .written_places()
            .iter()
            .map(|p| p.to_string())
            .collect();
        for field in &descriptor.affected_fields {
            assert!(
                written.contains(&format!("block.{field}")),
                "op {:?} declares #[affects({field:?})] but its #[emits] does not cover \
                 block.{field} — two declarations of what one op writes have drifted \
                 (ADR 0031 OQ-2). Declared writes: {written:?}",
                descriptor.name
            );
            checked += 1;
        }
    }

    // The lock's own coverage, ENFORCED rather than printed. A println is
    // invisible under nextest's default capture, so a lock that silently
    // checked nothing would read as a passing gate.
    //
    // Today `checked` is legitimately 0: P3 admits ops one at a time and
    // `set_field` — the first — carries no `#[affects]` (its affected field is
    // a runtime parameter, as its own doc comment says). The moment a declared
    // op DOES carry `#[affects]`, zero pairs means the loop stopped working,
    // and the assert below flips from disclosure to enforcement on its own.
    let declared_with_affects: Vec<&str> = catalog
        .iter()
        .filter(|d| matches!(d.arcs, TransitionArcs::Declared { .. }))
        .filter(|d| !d.affected_fields.is_empty())
        .map(|d| d.name.as_str())
        .collect();

    assert_eq!(
        checked == 0,
        declared_with_affects.is_empty(),
        "zero affects↦emits pairs were checked, but these declared ops DO carry \
         #[affects]: {declared_with_affects:?}. The lock reported success without \
         comparing anything."
    );

    if checked == 0 {
        // Not a failure — but it must be visible. stderr survives nextest
        // capture on failure and `--nocapture` always; the test NAME carries
        // the state for anyone reading a green run's list.
        eprintln!(
            "[arc-affects-lock] VACUOUS: 0 affects↦emits pairs checked. No declared op \
             carries #[affects] yet. The mechanism is proven instead by \
             `declared_emits_cover_the_declared_affects` in holon-macros-test."
        );
    }
}

/// Names the vacuity so it is legible in a green test list, and fails the
/// moment the disclosure stops being true — at which point this test should be
/// deleted and the lock above becomes the whole story.
#[tokio::test(flavor = "multi_thread")]
async fn arc_affects_lock_is_still_vacuous_over_the_production_catalog() {
    let engine = block_engine().await;
    let catalog = engine.available_operations(BLOCK).await;

    let declared_with_affects: Vec<&str> = catalog
        .iter()
        .filter(|d| matches!(d.arcs, TransitionArcs::Declared { .. }))
        .filter(|d| !d.affected_fields.is_empty())
        .map(|d| d.name.as_str())
        .collect();

    assert!(
        declared_with_affects.is_empty(),
        "the lock is no longer vacuous — {declared_with_affects:?} now declare BOTH \
         #[affects] and arcs. Delete this test: the disclosure it carries is stale, \
         and `every_declared_ops_emits_covers_its_affects` is now doing real work."
    );
}

/// Non-vacuity of the OTHER direction: the first declared op must actually be
/// in the catalog carrying arcs, or the loop above skips everything and the
/// lock is a no-op that nobody notices.
#[tokio::test(flavor = "multi_thread")]
async fn the_declared_set_is_not_empty() {
    let engine = block_engine().await;
    let catalog = engine.available_operations(BLOCK).await;

    let declared: Vec<&str> = catalog
        .iter()
        .filter(|d| matches!(d.arcs, TransitionArcs::Declared { .. }))
        .map(|d| d.name.as_str())
        .collect();

    assert!(
        declared.contains(&"set_field"),
        "set_field is the first op admitted to the ADR 0031 exhaustiveness set (OQ-4); \
         declared ops in the block catalog: {declared:?}"
    );
}
