//! Boundary-behavior ↔ registry correspondence lock (ADR 0028 C3, Increment 1).
//!
//! The sibling of `slashmenu_correspondence.rs`: where that oracle asserts the
//! rendered menu equals the `Listed` ops, this one asserts every STRUCTURAL
//! block op in the real catalog carries an explicit, non-`Unclassified`
//! [`BoundaryBehavior`].
//!
//! The `#[operations_trait]` macro emits the fail-closed
//! `BoundaryBehavior::Unclassified` when `#[boundary_behavior(...)]` is absent.
//! `Unclassified`'s runtime meaning is "any boundary interaction is REJECTED
//! loudly" — safe, but a genuinely-crossing structural op left `Unclassified`
//! would be a bug (it could never actually cross). This test is the backstop
//! that a new structural block op cannot ship unclassified: it feeds the SAME
//! `BackendEngine::available_operations("block")` catalog the profile resolver
//! and MCP discovery use, and fails loudly, naming the offenders.

use std::sync::Arc;

use holon::api::BackendEngine;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::BoundaryBehavior;
use holon_api::block::Block;
use holon_core::OperationProvider;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

const BLOCK: &str = "block";

/// Production SqlOnly block wiring (CRUD authority `SqlOperationProvider` +
/// structural provider `SqlBlockOperations`), identical to the
/// `slashmenu_correspondence` precedent — so `available_operations("block")`
/// returns the SAME catalog a live block's profile carries: macro CRUD/task/
/// structural descriptors + the bespoke `dismiss_advice`/`add_tag`/`remove_tag`
/// + the `convert_block_to_page` / `instantiate_template` synthetics.
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
async fn every_structural_block_op_has_an_explicit_boundary_behavior() {
    let engine = block_engine().await;
    let catalog = engine.available_operations(BLOCK).await;

    // Non-vacuity: a broken assembly returning [] must not pass by default.
    assert!(
        !catalog.is_empty(),
        "block catalog must be non-empty — a vacuous catalog would let the \
         exhaustiveness lock pass trivially"
    );

    // The lock: no structural block op ships with the fail-closed default.
    let unclassified: Vec<String> = catalog
        .iter()
        .filter(|d| matches!(d.boundary_behavior, BoundaryBehavior::Unclassified))
        .map(|d| d.name.clone())
        .collect();
    assert!(
        unclassified.is_empty(),
        "structural block ops shipped WITHOUT #[boundary_behavior(...)] — the \
         macro left them fail-closed `Unclassified`, so a genuine sharing-boundary \
         crossing would be silently mishandled (ADR 0028 C3). Classify each: {unclassified:?}"
    );

    // Witnesses that the classification is real and directional, not a blanket
    // constant that would also satisfy the lock above.
    let by_name = |n: &str| {
        catalog
            .iter()
            .find(|d| d.name == n)
            .unwrap_or_else(|| {
                let names: Vec<_> = catalog.iter().map(|d| d.name.clone()).collect();
                panic!("expected op {n:?} in the block catalog; got {names:?}")
            })
            .boundary_behavior
            .clone()
    };

    assert_eq!(
        by_name("indent"),
        BoundaryBehavior::Crossing {
            widens_audience: true
        },
        "indent reparents a block and can move it into a shared subtree — a \
         potentially-widening crossing"
    );
    assert_eq!(
        by_name("outdent"),
        BoundaryBehavior::ForbiddenAtPageBoundary,
        "outdent carries the D1 page-child rejection — forbidden at a page boundary"
    );
    assert_eq!(
        by_name("set_field"),
        BoundaryBehavior::PrivateOnly,
        "set_field is a within-container content edit — never crosses an audience \
         boundary"
    );
}
