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
pub mod document_entity;
pub mod document_operations;
pub mod event_bus;
pub mod event_subscriber;
#[cfg(feature = "iroh-sync")]
pub mod iroh_sync_adapter;
pub mod live_data;
pub mod loro_block_operations;
pub mod loro_blocks_datasource;
pub mod loro_document;
pub mod loro_document_store;
pub mod loro_event_adapter;
pub mod loro_module;
pub mod matview_manager;
pub mod turso_command_log;
pub mod turso_event_bus;

pub use cache_event_subscriber::CacheEventSubscriber;
pub use canonical_path::CanonicalPath;
pub use command_log::*;
pub use document_entity::Document;
pub use document_operations::DocumentOperations;
pub use event_bus::*;
pub use event_subscriber::EventSubscriber;
pub use holon_api::EntityUri;
#[cfg(feature = "iroh-sync")]
pub use iroh_sync_adapter::IrohSyncAdapter;
pub use live_data::LiveData;
pub use loro_block_operations::LoroBlockOperations;
pub use loro_blocks_datasource::LoroBlocksDataSource;
pub use loro_document::*;
pub use loro_document_store::*;
pub use loro_event_adapter::LoroEventAdapter;
pub use loro_module::{LoroConfig, LoroEventAdapterHandle, LoroModule};
pub use matview_manager::{MatviewManager, WatchResult};
pub use turso_command_log::TursoCommandLog;
pub use turso_event_bus::TursoEventBus;
