//! Core traits for Holon datasources
//!
//! This crate provides the core traits for datasource operations:
//! - `CrudOperations`: Basic CRUD operations (create, update, delete)
//! - `BlockOperations`: Block-specific operations (indent, outdent, move_block, etc.)
//! - `TaskOperations`: Task-specific operations (set_state, set_priority, set_due_date)

pub mod core;
pub mod fractional_index;
pub mod operation_log;
pub mod storage;
pub mod traits;
pub mod undo;

#[cfg(test)]
mod block_operations_tests;

pub use operation_log::{OperationLogEntry, OperationStatus};
pub use traits::{
    BlockDataSourceHelpers, BlockEntity, BlockMaintenanceHelpers, BlockOperations,
    BlockQueryHelpers, CompletionStateInfo, CrudOperations, DataSource, EditorCursorOperations,
    FieldDelta, MarkOperations, MaybeSendSync, MoveOperations, OperationLogOperations,
    OperationRegistry, OperationResult, RenameOperations, Result, TaskEntity, TaskOperations,
    TextOperations, UndoAction, UnknownOperationError,
};
pub use undo::UndoStack;

// Re-export macro-generated operation dispatch functions
pub use traits::{
    __operations_block_operations, __operations_crud_operations,
    __operations_editor_cursor_operations, __operations_mark_operations,
    __operations_move_operations, __operations_rename_operations, __operations_task_operations,
    __operations_text_operations,
};
