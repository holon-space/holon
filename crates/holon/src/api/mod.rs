//! Shared API crate for holon frontends
//!
//! This crate provides technology-agnostic types and traits for all
//! holon frontends (Tauri, Flutter, future REST API, etc.).
//!
//! # Architecture
//!
//! - `types`: Core data types (Block, InitialState, ApiError, etc.)
//! - `repository`: DocumentRepository trait defining backend operations
//! - `backend_engine`: PRQL render engine for reactive UI (Phase 4.1)
//! - `ffi_bridge`: FFI functions exposed to Flutter (Phase 4.1)
//!
//! # Design Principles
//!
//! - Technology-agnostic: No frontend-specific dependencies
//! - Clean domain model: Hides CRDT implementation details
//! - Type-safe errors: Structured error handling across FFI boundaries
//! - Async-first: All operations return Futures for flexibility

pub mod action_watcher;
// event_ring moved to holon-loro; re-exported below
// loro_backend moved to holon-loro; re-exported below
pub mod memory_backend;
// pbt_infrastructure pulls in proptest which is native-only.
// Gated behind `testing` (or #[cfg(test)]): zero production consumers.
#[cfg(all(not(target_arch = "wasm32"), any(test, feature = "testing")))]
pub mod pbt_infrastructure;
pub mod repository;
pub mod types;

pub mod backend_engine;
pub mod block_domain;
pub mod holon_service;
pub mod loro_ui_watcher;
pub mod operation_dispatcher;
pub mod operation_engine;
pub mod query_engine;
pub mod rule_status;
pub mod ui_watcher;

// `SnapshotBlock` is a pure-data API type (it lives in `holon-api`, not a
// backend); re-export it from its real home for the watcher/sink layer.
// `LoroBackend` and the Loro snapshot readers are no longer re-exported here —
// consumers name `holon_loro::` directly (Phase 3b decoupling). The
// `holon::sync::*` glob (sync/mod.rs) remains its own deferred migration slice.
// Re-export render engine types for FFI
pub use backend_engine::BackendEngine;
pub use block_domain::BlockDomain;
pub use holon_api::SnapshotBlock;
// Re-export streaming types from holon-api (moved from streaming module)
pub use holon_api::{
    ApiError, Batch, BatchMapChange, BatchMetadata, BatchTraceContext, BatchWithMetadata, Block,
    BlockChange, BlockMetadata, Change, ChangeOrigin, MapChange, StreamPosition, WithMetadata,
};
// Re-export OperationDescriptor and OperationParam for FRB type generation
pub use holon_api::{OperationDescriptor, OperationParam};
pub use holon_service::HolonService;
pub use memory_backend::MemoryBackend;
pub use operation_dispatcher::OperationDispatcher;
pub use operation_engine::DispatchingOperationEngine;
pub use operation_engine::OperationEngine;
pub use query_engine::QueryEngine;
pub use query_engine::SqlQueryEngine;
pub use repository::CoreOperations;
pub use repository::DocumentRepository;
pub use repository::Lifecycle;
pub use repository::P2POperations;
pub use ui_watcher::UiWatcher;
pub use ui_watcher::watch_ui;

// Re-export CDC streaming types
pub use crate::storage::turso::{ChangeData, RowChange, RowChangeStream};
