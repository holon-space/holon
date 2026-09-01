//! @c4 component
//! @c4 layer Core
//! Pattern: Port
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-macros "entity/operation derive macros" "Rust"
//!
//! Core traits for Holon datasources
//!
//! This crate provides the core traits for datasource operations:
//! - `CrudOperations`: Basic CRUD operations (create, update, delete)
//! - `BlockOperations`: Block-specific operations (indent, outdent, move_block,
//!   etc.)
//! - `TaskOperations`: Task-specific operations (set_state, set_priority,
//!   set_due_date)

pub mod block_op_catalog;
pub mod block_ordering;
pub mod boundary_enforcer;
pub mod canonical_path;
pub mod cell;
pub mod cell_registry;
pub mod core;
pub mod downstream_projection;
pub mod entity_cache;
pub mod file_format;
pub mod fractional_index;
pub mod integration_attribution;
pub mod memstats;
pub mod operation_log;
pub mod operation_subset;
pub mod operation_wrapper;
pub mod publish_errors;
pub mod replica_state;
pub mod sink_reader;
pub mod sql_cell_registry;
pub mod storage;
pub mod sync_gate;
pub mod traits;
pub mod undo;
pub mod util;

pub use boundary_enforcer::BoundaryEnforcer;
pub use boundary_enforcer::BoundaryRejection;
pub use boundary_enforcer::InertBoundaryEnforcer;
pub use canonical_path::CanonicalPath;
pub use downstream_projection::DownstreamProjection;
pub use entity_cache::CacheFactory;
pub use entity_cache::EntityCache;
pub use file_format::FileFormatAdapter;
pub use file_format::FileFormatParseResult;
pub use publish_errors::PublishErrorTracker;
pub use sink_reader::SinkReader;
pub use sql_cell_registry::SqlOnlyCellRegistry;
pub use sql_cell_registry::SqlScalarWriteFn;
pub use sync_gate::SyncGate;
pub use sync_gate::SyncGateClosed;
pub use sync_gate::SyncGateState;
pub use sync_gate::SyncGateWatcher;

#[cfg(test)]
mod block_operations_tests;

pub use operation_log::OperationLogEntry;
pub use operation_log::OperationStatus;
pub use operation_subset::OperationSubset;
pub use operation_wrapper::OperationWrapper;
pub use traits::BatchOp;
pub use traits::BlockDataSourceHelpers;
pub use traits::BlockEntity;
pub use traits::BlockOperations;
pub use traits::BlockQueryHelpers;
pub use traits::CompletionStateInfo;
pub use traits::CrudAuthority;
pub use traits::CrudOperations;
pub use traits::DataSource;
pub use traits::Delivery;
pub use traits::DeltaFingerprint;
pub use traits::EventOrigin;
pub use traits::FieldDelta;
pub use traits::MarkOperations;
pub use traits::MatviewHook;
pub use traits::MaybeSendSync;
pub use traits::MoveOperations;
pub use traits::NetAdmission;
pub use traits::OperationLogOperations;
pub use traits::OperationObserver;
pub use traits::OperationProvider;
pub use traits::OperationRegistry;
pub use traits::OperationResult;
pub use traits::OriginTaggedWrites;
pub use traits::RenameOperations;
pub use traits::Result;
pub use traits::StreamProvider;
pub use traits::SyncTokenStore;
pub use traits::SyncableProvider;
pub use traits::TaskEntity;
pub use traits::TaskOperations;
pub use traits::TextOperations;
pub use traits::UndoAction;
pub use traits::UnknownOperationError;
pub use traits::classify_for_net;
pub use traits::combine_matview_hooks;
pub use traits::generate_sync_operation;
pub use traits::has_sync_fan_out_name;
// Re-export macro-generated operation dispatch functions
pub use traits::{
    __operations_block_operations, __operations_crud_operations, __operations_mark_operations,
    __operations_move_operations, __operations_rename_operations, __operations_task_operations,
    __operations_text_operations,
};
pub use undo::FieldFingerprint;
pub use undo::Precondition;
pub use undo::UndoEntry;
pub use undo::UndoStack;
pub use undo::UndoStateReader;
pub use undo::UndoStore;
pub use undo::verify_precondition;
