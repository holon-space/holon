//! Event infrastructure DI module.
//!
//! Registers the shared event pipeline services needed by both Loro and OrgMode:
//! - `QueryableCache<Block>` — in-memory block cache fed by CDC
//! - `TursoEventBus` — event bus wired to Turso CDC
//! - `PublishErrorTracker` — tracks publish errors
//! - `CacheEventSubscriberHandle` — marker that triggers EventBus → block cache wiring
//!
//! Dir/file cache subscriptions are handled by the frontend layer (which has
//! access to `holon-filesystem` types) via the `extra_cache_subscribers` callback.

use fluxdi::{Injector, Module, Provider, Shared};
use std::sync::Arc;

use crate::core::datasource::OperationProvider;
use crate::core::queryable_cache::QueryableCache;
use crate::core::sql_block_operations::SqlBlockOperations;
use crate::core::sql_operation_provider::SqlOperationProvider;
use crate::di::DbHandleProvider;
use crate::storage::schema_module::SchemaModule;
use crate::storage::schema_modules::BlockSchemaModule;
use crate::sync::PublishErrorTracker;
use crate::sync::cache_event_subscriber::CacheEventSubscriber;
use crate::sync::event_bus::EventBus;
use crate::sync::link_event_subscriber::LinkEventSubscriber;
use crate::sync::turso_event_bus::{TursoEventBus, WatermarkState};
use holon_api::block::Block;

/// Marker type for the CacheEventSubscriber background wiring.
/// Resolving this from DI triggers the EventBus → QueryableCache subscription.
pub struct CacheEventSubscriberHandle;

/// Marker type for the LinkEventSubscriber background wiring.
/// Resolving this from DI triggers the EventBus → block_link table subscription.
pub struct LinkEventSubscriberHandle;

/// DI module for shared event infrastructure.
///
/// Register this module when any data pipeline is active (Loro, OrgMode, etc.).
/// It provides the EventBus, block cache, and error tracker that all pipelines share.
///
/// Only wires the block cache subscription. Dir/file cache subscriptions must be
/// set up by the caller after resolving `CacheEventSubscriberHandle` (since
/// `holon-filesystem` types are not available in this crate).
pub struct EventInfraModule;

impl Module for EventInfraModule {
    fn configure(&self, injector: &Injector) -> std::result::Result<(), fluxdi::Error> {
        injector.provide::<QueryableCache<Block>>(Provider::root_async(|r| async move {
            Shared::new(crate::di::create_queryable_cache_async(&r).await)
        }));

        injector.provide::<TursoEventBus>(Provider::root_async(|resolver| async move {
            let db_handle_provider = resolver.resolve::<dyn DbHandleProvider>();
            let db_handle = db_handle_provider.handle();
            TursoEventBus::init_schema(&db_handle)
                .await
                .expect("Failed to initialize EventBus schema");
            let watermark_state = WatermarkState::start(&db_handle)
                .await
                .expect("Failed to start WatermarkState");
            Shared::new(TursoEventBus::new(db_handle, watermark_state))
        }));

        injector.provide(Provider::root(move |_| {
            Shared::new(PublishErrorTracker::new())
        }));

        injector.provide::<CacheEventSubscriberHandle>(Provider::root_async(
            |resolver| async move {
                let block_cache = resolver.resolve_async::<QueryableCache<Block>>().await;
                let event_bus = resolver.resolve_async::<TursoEventBus>().await;
                let event_bus_arc: std::sync::Arc<dyn crate::sync::event_bus::EventBus> =
                    event_bus.clone();

                let subscriber = CacheEventSubscriber::new(block_cache);
                if let Err(e) = subscriber.start(event_bus_arc).await {
                    tracing::error!(
                        "[EventInfraModule] Failed to start CacheEventSubscriber: {}",
                        e
                    );
                }

                Shared::new(CacheEventSubscriberHandle)
            },
        ));

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
                let event_bus = resolver.resolve_async::<TursoEventBus>().await;
                let event_bus_arc: Arc<dyn EventBus> = event_bus.clone();
                let block_cache = resolver.resolve_async::<QueryableCache<Block>>().await;

                let sql_ops = Arc::new(SqlOperationProvider::with_event_bus_and_edge_fields(
                    db_handle_provider.handle(),
                    "block".to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    event_bus_arc,
                    BlockSchemaModule.edge_fields(),
                ));

                Arc::new(SqlBlockOperations::new(sql_ops, block_cache))
                    as Arc<dyn OperationProvider>
            },
        ));

        injector.provide::<LinkEventSubscriberHandle>(Provider::root_async(
            |resolver| async move {
                let db_handle_provider = resolver.resolve::<dyn DbHandleProvider>();
                let event_bus = resolver.resolve_async::<TursoEventBus>().await;
                let event_bus_arc: std::sync::Arc<dyn crate::sync::event_bus::EventBus> =
                    event_bus.clone();

                let subscriber = LinkEventSubscriber::new(db_handle_provider.handle());
                if let Err(e) = subscriber.start(event_bus_arc).await {
                    tracing::error!(
                        "[EventInfraModule] Failed to start LinkEventSubscriber: {}",
                        e
                    );
                }

                Shared::new(LinkEventSubscriberHandle)
            },
        ));

        Ok(())
    }
}
