//! Anti-laundering lock: no operation descriptor emits into authorization
//! state.
//!
//! `membership.*` and `session.*` record who may act
//! ([`holon_api::arcs::AUTHORITY_RESERVED_RELATIONS`]); their sole writer is
//! the sharing ingress. A descriptor that declares an out-arc or a
//! token-moving marking delta into them would mint capabilities from inside
//! the operation catalog. The sibling of
//! `boundary_behavior_correspondence.rs`: the same real
//! `available_operations("block")` catalog, scanned against the reservation.

use std::sync::Arc;

use holon::api::BackendEngine;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::arcs::TransitionArcs;
use holon_api::block::Block;
use holon_core::OperationProvider;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

const BLOCK: &str = "block";

/// Production SqlOnly block wiring, identical to the
/// `boundary_behavior_correspondence` precedent — so the scanned catalog is
/// the one a live block's profile carries.
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
async fn no_block_op_emits_into_authority_reserved_places() {
    let engine = block_engine().await;
    let catalog = engine.available_operations(BLOCK).await;

    // Non-vacuity: the scan must walk real declarations, not an empty or
    // undeclared catalog.
    assert!(
        !catalog.is_empty(),
        "block catalog must be non-empty — a vacuous catalog would let the \
         reservation lock pass trivially"
    );
    assert!(
        catalog.iter().any(|d| matches!(
            &d.arcs,
            TransitionArcs::Declared { emits, .. } if !emits.is_empty()
        )),
        "no block op declares any out-arc — the emit scan below would be vacuous"
    );
    assert!(
        catalog.iter().any(|d| d
            .marking_delta
            .kinds()
            .is_some_and(|ks| ks.iter().any(holon_api::marking::KindDelta::moves_tokens))),
        "no block op declares a token-moving marking delta — the delta scan below \
         would be vacuous"
    );

    // The lock: reads of authorization state are legal, emits never are.
    let offenders: Vec<String> = catalog
        .iter()
        .flat_map(|d| {
            let arc_hits = d
                .arcs
                .authority_reserved_emits()
                .into_iter()
                .map(move |p| format!("{}: emits into `{p}`", d.name));
            let delta_hits = d
                .marking_delta
                .authority_reserved_writes()
                .into_iter()
                .map(move |r| format!("{}: marking delta moves `{r}` tokens", d.name));
            arc_hits.chain(delta_hits).collect::<Vec<_>>()
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "operation descriptors declare writes into authority-reserved places \
         (`membership.*` / `session.*`) — only the sharing ingress may write \
         authorization state; an op emitting into it mints capabilities: {offenders:?}"
    );
}
