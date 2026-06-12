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
// Also gated behind test-helpers: zero production consumers outside #[cfg(test)].
#[cfg(all(not(target_arch = "wasm32"), any(test, feature = "test-helpers")))]
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
pub mod ui_watcher;

// Re-export commonly used types
pub use holon_loro::{
    LoroBackend, SnapshotBlock, snapshot_blocks_from_doc, snapshot_blocks_from_doc_settled,
};

// Backward compatibility: holon::api::loro_backend::TREE_NAME etc. keep resolving
pub use holon_loro as loro_backend;
pub use memory_backend::MemoryBackend;
pub use repository::{CoreOperations, DocumentRepository, Lifecycle, P2POperations};
// Re-export streaming types from holon-api (moved from streaming module)
pub use holon_api::{
    ApiError, Batch, BatchMapChange, BatchMetadata, BatchTraceContext, BatchWithMetadata, Block,
    BlockChange, BlockMetadata, BlockWithDepth, Change, ChangeOrigin, MapChange, StreamPosition,
    WithMetadata,
};

// Re-export render engine types for FFI
pub use backend_engine::BackendEngine;
pub use block_domain::BlockDomain;
pub use holon_service::HolonService;
pub use operation_dispatcher::OperationDispatcher;
pub use operation_engine::{DispatchingOperationEngine, OperationEngine};
pub use query_engine::{QueryEngine, SqlQueryEngine};
pub use ui_watcher::{UiWatcher, watch_ui};

// Re-export OperationDescriptor and OperationParam for FRB type generation
pub use holon_api::{OperationDescriptor, OperationParam};

// Re-export CDC streaming types
pub use crate::storage::turso::{ChangeData, RowChange, RowChangeStream};
