//! Synchronization infrastructure
//!
//! - `canonical_path`: Type-safe canonical path that resolves symlinks
//! - `loro_document`: Loro CRDT document (storage only, no transport)
//! - `iroh_sync_adapter`: Iroh P2P transport adapter for syncing Loro documents
//! - `external_system`: External system integration with contract-based validation
//! - `loro_document_store`: Store for managing multiple Loro documents
//! - `loro_block_operations`: Generic operations on Loro blocks
//! - `loro_blocks_datasource`: DataSource for populating QueryableCache
//! - `event_bus`: Event bus trait and types for event sourcing
//! - `command_log`: Command log trait and types for persistent undo/redo
//!
//! Note: Block hierarchy schema is now managed by `BlockHierarchySchemaModule`
//! in `storage/schema_modules.rs` via the `SchemaRegistry`.

pub mod cache_event_subscriber;
pub mod canonical_path;

pub mod command_log;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub mod debounced_commit_worker;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub mod degraded_signal_bus;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub mod device_key_store;
pub mod event_bus;
pub mod event_infra_module;
pub mod event_subscriber;
#[cfg(test)]
mod fork_at_test;
#[cfg(test)]
mod inbound_parent_id_test;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub mod iroh_advertiser;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub mod iroh_sync_adapter;
pub mod link_event_subscriber;
pub mod live_data;
pub mod live_value;
pub mod loro_block_operations;
pub mod loro_blocks_datasource;
pub mod loro_document;
pub mod loro_document_store;
pub mod loro_module;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub mod loro_share_backend;
pub mod loro_sync_controller;
pub mod matview_manager;
#[cfg(any(test, feature = "test-helpers"))]
pub mod multi_peer;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub mod share_peer_id;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub mod shared_snapshot_store;
pub mod shared_tree;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub mod ticket;
pub mod turso_command_log;
pub mod turso_event_bus;

pub use cache_event_subscriber::CacheEventSubscriber;
pub use canonical_path::CanonicalPath;
pub use command_log::*;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub use degraded_signal_bus::{DegradedSignalBus, ShareDegraded, ShareDegradedReason};
pub use event_bus::*;
pub use event_infra_module::{
    CacheEventSubscriberHandle, EventInfraModule, LinkEventSubscriberHandle,
};
pub use event_subscriber::EventSubscriber;
pub use holon_api::EntityUri;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub use iroh_sync_adapter::IrohSyncAdapter;
pub use live_data::LiveData;
pub use live_value::LiveValue;
pub use loro_block_operations::LoroBlockOperations;
pub use loro_blocks_datasource::LoroBlocksDataSource;
pub use loro_document::*;
pub use loro_document_store::*;
pub use loro_module::{LoroConfig, LoroModule};
pub use loro_sync_controller::{LoroSyncController, LoroSyncControllerHandle};
pub use matview_manager::{MatviewHook, MatviewManager, WatchResult, reconcile_named_view};
pub use turso_command_log::TursoCommandLog;
pub use turso_event_bus::TursoEventBus;
