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
//! - `BlockOperations`: Block-specific operations (indent, outdent, move_block, etc.)
//! - `TaskOperations`: Task-specific operations (set_state, set_priority, set_due_date)

pub mod block_ordering;
pub mod canonical_path;
pub mod cell;
pub mod cell_registry;
pub mod core;
pub mod downstream_projection;
pub mod file_format;
pub mod fractional_index;
pub mod operation_log;
pub mod publish_errors;
pub mod storage;
pub mod traits;
pub mod undo;
pub mod util;

pub use canonical_path::CanonicalPath;
pub use downstream_projection::DownstreamProjection;
pub use file_format::{FileFormatAdapter, FileFormatParseResult};
pub use publish_errors::PublishErrorTracker;

#[cfg(test)]
mod block_operations_tests;

pub use operation_log::{OperationLogEntry, OperationStatus};
pub use traits::{
    BlockDataSourceHelpers, BlockEntity, BlockMaintenanceHelpers, BlockOperations,
    BlockQueryHelpers, CompletionStateInfo, CrudAuthority, CrudOperations, DataSource, EventOrigin,
    FieldDelta, MarkOperations, MaybeSendSync, MoveOperations, OperationLogOperations,
    OperationProvider, OperationRegistry, OperationResult, OriginTaggedWrites, RenameOperations,
    Result, TaskEntity, TaskOperations, TextOperations, UndoAction, UnknownOperationError,
};
pub use undo::UndoStack;

// Re-export macro-generated operation dispatch functions
pub use traits::{
    __operations_block_operations, __operations_crud_operations, __operations_mark_operations,
    __operations_move_operations, __operations_rename_operations, __operations_task_operations,
    __operations_text_operations,
};
