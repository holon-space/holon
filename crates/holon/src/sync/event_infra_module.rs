//! Event infrastructure DI module.
//!
//! Registers the shared block pipeline services needed by both Loro and OrgMode:
//! - `QueryableCache<Block>` — in-memory block cache fed by CDC
//! - `BlockFeed` — the convergent `LiveData<Block>` over the `block` matview
//! - `PublishErrorTracker` — tracks publish errors
//!
//! The block cache is a thin SQL view fed by the shared Turso connection
//! directly; downstream sinks (Loro→SQL controller, link indexer) react to the
//! shared `BlockFeed`.

use fluxdi::{Injector, Module, Provider, Shared};
use std::sync::Arc;

use crate::core::queryable_cache::QueryableCache;
use crate::core::sql_block_operations::SqlBlockOperations;
use crate::core::sql_operation_provider::SqlOperationProvider;
use crate::di::DbHandleProvider;
use crate::storage::BLOCK_WRITE_TABLE;
use crate::storage::schema_module::SchemaModule;
use crate::storage::schema_modules::BlockSchemaModule;
use crate::storage::turso_block_link_indexer::TursoBlockLinkIndexer;
use crate::sync::PublishErrorTracker;
use crate::sync::link_event_subscriber::LinkEventSubscriber;
use crate::sync::live_data::LiveData;
use holon_api::block::Block;
use holon_api::capability::SessionCapabilities;
use holon_api::{EntityName, EntityUri, Value};
use holon_core::block_ordering::BlockOrdering;
use holon_core::{OperationProvider, OperationRegistry};

/// Build the SqlOnly cell `set_field` write path over `SqlOperationProvider`.
///
/// Routed straight to the SQL `set_field` operation (mirroring the fall-through
/// branch of `SqlBlockOperations::set_field`, which is where every SqlOnly block
/// write already lands). Deliberately does NOT go back through
/// `BlockCellRegistry::write_field` / `SqlBlockOperations`: the registry is owned
/// by `SqlBlockOperations`, so routing through it would form an `Arc` cycle.
fn sql_cell_set_field_writer(
    sql_ops: Arc<SqlOperationProvider>,
) -> crate::sync::block_cell_registry::SqlScalarWriteFn {
    Arc::new(move |uri: EntityUri, field: String, value: Value| {
        let sql_ops = sql_ops.clone();
        Box::pin(async move {
            let mut params: holon_core::storage::types::StorageEntity =
                std::collections::HashMap::new();
            params.insert("id".into(), Value::String(uri.to_string()));
            params.insert("field".into(), Value::String(field));
            params.insert("value".into(), value);
            let entity = EntityName::new(Block::entity_name());
            sql_ops
                .execute_operation(&entity, "set_field", params)
                .await
                .map(|_| ())
                .map_err(|e| anyhow::anyhow!("SqlOnly cell set_field: {e}"))
        }) as futures::future::BoxFuture<'static, anyhow::Result<()>>
    })
}

/// Marker type for the LinkEventSubscriber background wiring.
/// Resolving this from DI triggers the LiveData<Block> → block_link subscription.
pub struct LinkEventSubscriberHandle;

/// The shared convergent block feed (`LiveData<Block>` over the `block`
/// matview's CDC stream). Built once and shared by every downstream sink that
/// needs it (the Loro→SQL controller keeps it alive; the link indexer drives
/// `block_link` off it). A newtype so DI hands back the inner `Arc` cleanly
/// (`LiveData::new` already returns an `Arc`). Available in both modes — the
/// matview exists with or without Loro.
pub struct BlockFeed(pub Arc<LiveData<Block>>);

/// DI module for shared block infrastructure.
///
/// Register this module when any data pipeline is active (Loro, OrgMode, etc.).
/// It provides the block cache, the shared `BlockFeed`, and the error tracker
/// that all pipelines share. The block cache is fed directly from the shared
/// Turso connection, not via any event bus.
pub struct EventInfraModule;

impl Module for EventInfraModule {
    fn configure(&self, injector: &Injector) -> std::result::Result<(), fluxdi::Error> {
        injector.provide::<QueryableCache<Block>>(Provider::root_async(|r| async move {
            Shared::new(crate::di::create_queryable_cache_async(&r).await)
        }));

        injector.provide(Provider::root(move |_| {
            Shared::new(PublishErrorTracker::new())
        }));

        // Shared convergent block feed (`LiveData<Block>` over the `block`
        // matview CDC). Built once; downstream sinks resolve and share it.
        injector.provide::<BlockFeed>(Provider::root_async(|resolver| async move {
            let matview_manager = resolver.resolve::<crate::sync::MatviewManager>();
            let watch = matview_manager
                .watch("SELECT * FROM block")
                .await
                .expect("[EventInfraModule] watch block matview for BlockFeed");
            let live = LiveData::new(
                watch.initial_rows,
                |row: &holon_core::storage::types::StorageEntity| {
                    row.get("id")
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_string())
                        .ok_or_else(|| anyhow::anyhow!("block matview row missing id"))
                },
                |row: &holon_core::storage::types::StorageEntity| Block::try_from(row.clone()),
            );
            live.subscribe("block", watch.stream);
            Shared::new(BlockFeed(live))
        }));

        // Register SqlBlockOperations as a separate OperationProvider for the
        // "block" entity, declaring `BlockOperations` (indent / outdent /
        // move_block / move_up / move_down / split_block). Without this,
        // nothing in the dispatcher answers an `indent` request — the keychord
        // registered for Tab in the frontend cannot bind to any widget. See
        // crates/holon/src/core/sql_block_operations.rs for the full
        // diagnosis. Reads come from the same QueryableCache<Block> as the
        // rest of the system; writes route through SqlOperationProvider so SQL
        // remains the source of truth.
        injector.provide_into_set::<dyn OperationProvider>(Provider::root_async(
            |resolver| async move {
                let db_handle_provider = resolver.resolve::<dyn DbHandleProvider>();
                let block_cache = resolver.resolve_async::<QueryableCache<Block>>().await;

                let sql_ops = Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle_provider.handle(),
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    BlockSchemaModule.edge_fields(),
                ));

                // Resolve the `BlockCellRegistry`. In Full mode (LoroModule
                // loaded) this is Loro-aware; in SqlOnly mode it's a
                // `BlockCellRegistry::sql_only()` that errors loudly on
                // every `live_field` lookup (no editor in SqlOnly mode).
                // Synthetic `MemStore` test impls bypass this entirely
                // because they inherit the `BlockOperations::cells()`
                // default of `None`.
                let cell_registry: Arc<crate::sync::block_cell_registry::BlockCellRegistry> =
                    match resolver
                        .optional_resolve_async::<crate::sync::block_cell_registry::BlockCellRegistry>()
                        .await
                    {
                        Some(arc) => arc.clone(),
                        None => {
                            // SqlOnly mode: wire cells to the convergent block
                            // feed (read + CDC signal) + the SQL set_field write
                            // path so `live_field` presents the same cell surface
                            // as Full (Loro) mode instead of erroring.
                            let live = resolver.resolve_async::<BlockFeed>().await;
                            Arc::new(
                                crate::sync::block_cell_registry::BlockCellRegistry::sql_only_wired(
                                    live.0.clone(),
                                    sql_cell_set_field_writer(sql_ops.clone()),
                                ),
                            )
                        }
                    };
                // The composition root knows what's present, so it resolves the
                // capability role here and injects it — `SqlBlockOperations`
                // never probes a Loro-aware component itself.
                let caps = SessionCapabilities::detect_and_pin(cell_registry.has_loro_backing());
                let block_ops = SqlBlockOperations::new(sql_ops, block_cache)
                    .with_cell_registry(cell_registry)
                    .with_capabilities(caps);
                Arc::new(block_ops) as Arc<dyn OperationProvider>
            },
        ));

        // Expose a shared BlockOrdering instance for consumers that need positional
        // intent writes (FileSyncController, future markdown adapter, MCP).
        // Uses the same SqlBlockOperations construction as the OperationProvider above,
        // routing through the cell_registry for Loro vs SqlOnly mode.
        injector.provide::<dyn BlockOrdering>(Provider::root_async(|resolver| async move {
            let db_handle_provider = resolver.resolve::<dyn DbHandleProvider>();
            let block_cache = resolver.resolve_async::<QueryableCache<Block>>().await;

            let sql_ops = Arc::new(SqlOperationProvider::with_edge_fields(
                db_handle_provider.handle(),
                BLOCK_WRITE_TABLE.to_string(),
                "block".to_string(),
                "block".to_string(),
                BlockSchemaModule.edge_fields(),
            ));

            let cell_registry: Arc<crate::sync::block_cell_registry::BlockCellRegistry> =
                match resolver
                    .optional_resolve_async::<crate::sync::block_cell_registry::BlockCellRegistry>()
                    .await
                {
                    Some(arc) => arc.clone(),
                    None => {
                        let live = resolver.resolve_async::<BlockFeed>().await;
                        Arc::new(
                            crate::sync::block_cell_registry::BlockCellRegistry::sql_only_wired(
                                live.0.clone(),
                                sql_cell_set_field_writer(sql_ops.clone()),
                            ),
                        )
                    }
                };
            let caps = SessionCapabilities::detect_and_pin(cell_registry.has_loro_backing());
            let block_ops = SqlBlockOperations::new(sql_ops, block_cache)
                .with_cell_registry(cell_registry)
                .with_capabilities(caps);
            Arc::new(block_ops) as Arc<dyn BlockOrdering>
        }));

        injector.provide::<LinkEventSubscriberHandle>(Provider::root_async(
            |resolver| async move {
                let db_handle_provider = resolver.resolve::<dyn DbHandleProvider>();
                let block_feed = resolver.resolve_async::<BlockFeed>().await;

                let indexer =
                    std::sync::Arc::new(TursoBlockLinkIndexer::new(db_handle_provider.handle()));
                let subscriber = LinkEventSubscriber::new(indexer);
                subscriber.start_from_live_data(block_feed.0.clone());

                Shared::new(LinkEventSubscriberHandle)
            },
        ));

        Ok(())
    }
}
