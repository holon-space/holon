//! Compose the SQL slice's SUT `CapMap` from its component — "a slice is a
//! component list" (§1). A third storage backing the *same* shared catalog,
//! after `memory_slice` (`MemoryBackend`) and `loro_slice` (`LoroBackend`).

use std::sync::Arc;

use fluxdi::Provider;
use holon::api::BackendEngine;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::di::DbHandleProvider;
use holon::di::test_helpers::create_test_engine_with_setup;
use holon::storage::BLOCK_WRITE_TABLE;
use holon::storage::schema_module::SchemaModule;
use holon::storage::schema_modules::BlockSchemaModule;
use holon_api::block::Block;
use holon_core::OperationProvider;
use holon_pbt_core::composition::{CapMap, Config};

use super::components::SqlProjectionComponent;

/// Build a Turso `BackendEngine` with the production block **CRUD** operation
/// provider registered — a `SqlOperationProvider` over the `block_raw` write
/// table with the production block edge fields, exactly as `EventInfraModule`
/// constructs it — but **no** reactive engine, frontend, navigation, or CDC
/// machinery. The storage-layer slice of the future `E2ESut` replacement.
///
/// Structural block ops (`indent`/`split_block`/…) live on `SqlBlockOperations`
/// (via `EventInfraModule`) and are out of this slice's scope; create / set_field
/// / delete suffice to drive the block-tree and SQL-projection invariants.
///
/// Shared by the SQL slice and the combined `sql_loro_slice` (one engine
/// builder, no copy-paste).
pub async fn new_sql_engine() -> Arc<BackendEngine> {
    create_test_engine_with_setup(":memory:".into(), |injector| {
        injector.provide_into_set::<dyn OperationProvider>(Provider::root_async(
            |resolver| async move {
                let dbh = resolver.resolve::<dyn DbHandleProvider>();
                Arc::new(SqlOperationProvider::with_edge_fields(
                    dbh.handle(),
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    BlockSchemaModule.edge_fields(),
                )) as Arc<dyn OperationProvider>
            },
        ));
        Ok(())
    })
    .await
    .expect("create sql test engine")
}

/// A Turso `BackendEngine` registering **both** the CRUD `SqlOperationProvider`
/// (create/set_field/delete) **and** the production `SqlBlockOperations`
/// (split_block/indent/outdent/move_up/move_down/join_block) — the structural-op
/// path `new_sql_engine` omits. The dispatcher routes by `(entity, op)` and the two
/// providers' op-name sets are disjoint, so they coexist for the `block` entity.
///
/// This is the async **structural write** substrate for composed slices that drive
/// the production block-tree alphabet through `OpDispatchWriter`
/// ([`crate::pbt::op_write_cap`]) — the storage-slice half of the `E2ESut`
/// replacement's write path.
pub async fn new_sql_engine_with_structural_ops() -> Arc<BackendEngine> {
    create_test_engine_with_setup(":memory:".into(), |injector| {
        // CRUD provider (create / set_field / delete).
        injector.provide_into_set::<dyn OperationProvider>(Provider::root_async(
            |resolver| async move {
                let dbh = resolver.resolve::<dyn DbHandleProvider>();
                Arc::new(SqlOperationProvider::with_edge_fields(
                    dbh.handle(),
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    BlockSchemaModule.edge_fields(),
                )) as Arc<dyn OperationProvider>
            },
        ));
        // Structural provider (split_block / indent / outdent / move_up / move_down /
        // join_block).
        injector.provide_into_set::<dyn OperationProvider>(Provider::root_async(
            |resolver| async move {
                let dbh = resolver.resolve::<dyn DbHandleProvider>();
                let sql_ops = Arc::new(SqlOperationProvider::with_edge_fields(
                    dbh.handle(),
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    BlockSchemaModule.edge_fields(),
                ));
                // Point the cache at `block_raw` (not the `block` matview) so writes
                // are immediately visible to the structural ops' ordering reads.
                let mut block_raw_type_def = Block::type_definition();
                block_raw_type_def.name = "block_raw".to_string();
                let cache = Arc::new(
                    QueryableCache::<Block>::new(dbh.handle(), block_raw_type_def)
                        .await
                        .expect("block_raw cache"),
                );
                Arc::new(SqlBlockOperations::new(sql_ops, cache)) as Arc<dyn OperationProvider>
            },
        ));
        Ok(())
    })
    .await
    .expect("create sql test engine with structural ops")
}

/// Build a structural-write composed SUT over a real Turso engine: `SutBackend`
/// (block-tree invariants) + the dispatch-floor [`DirectUserDriver`] as
/// `SutBlockTreeWrite` (§8.11 LL-2 — a storage-only config has no higher driver, so
/// the floor applies structural ops directly to the engine), sharing `resolver` (the
/// synthetic→real id map the composed runner accumulates). `SutSqlProjection` is
/// intentionally NOT registered so the navigation/focus invariants deselect (this
/// slice has no navigation engine; a focused oracle would false-RED against empty
/// focus rows).
pub fn sql_structural_wide(
    engine: Arc<BackendEngine>,
    resolver: crate::pbt::op_write_cap::IdResolver,
) -> CapMap {
    use holon_pbt_core::capabilities::{SutBackend, SutBlockTreeWrite};
    let mut caps = CapMap::new();
    caps.insert(Arc::new(SqlProjectionComponent::new(engine.clone())) as Arc<dyn SutBackend>);
    caps.insert(
        Arc::new(crate::pbt::op_write_cap::OpDispatchWriter::with_resolver(
            engine, resolver,
        )) as Arc<dyn SutBlockTreeWrite>,
    );
    caps
}

/// Build the SQL-storage composed SUT — a real Turso `BackendEngine` exposed as
/// `SutBackend` + `SutSqlProjection`. The same `composed_invariant_catalog()`
/// the memory and Loro slices run selects against these caps; every block-tree
/// invariant now validates the Turso realization too, and the SQL-projection
/// invariants light up by capability presence.
pub fn sql_wide(engine: Arc<BackendEngine>) -> CapMap {
    Config::new()
        .with(SqlProjectionComponent::new(engine))
        .build()
}
