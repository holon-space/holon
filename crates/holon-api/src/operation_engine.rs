//! The operation-execution capability trait (ADR 0004 — "Turso is one of
//! four").
//!
//! `OperationEngine` is the seam the frontend's mutation/operation path depends
//! on instead of a concrete backend. It covers dispatching operations,
//! discovering which operations an entity supports, and undo/redo. Operations
//! are *not* fundamentally Turso-bound — implementations live in
//! `holon::api::operation_engine` (`BackendEngine` and the dispatcher-backed
//! `DispatchingOperationEngine`); the frontend holds it as
//! `Option<Arc<dyn OperationEngine>>` so a session without the capability
//! reports its absence as a typed fact rather than panicking behind `engine()`.

use anyhow::Result;
use async_trait::async_trait;

use crate::EntityName;
use crate::OperationDescriptor;
use crate::StorageEntity;
use crate::Value;

/// Execute, discover, and undo/redo operations.
#[async_trait]
pub trait OperationEngine: Send + Sync {
    /// Dispatch an operation, returning its optional result value.
    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<Option<Value>>;

    /// The operations registered for `entity_name`.
    async fn available_operations(&self, entity_name: &str) -> Vec<OperationDescriptor>;

    /// Whether `op_name` is registered for `entity_name`.
    async fn has_operation(&self, entity_name: &str, op_name: &str) -> bool;

    /// Undo the last operation; `false` if the undo stack is empty.
    async fn undo(&self) -> Result<bool>;

    /// Redo the last undone operation; `false` if the redo stack is empty.
    async fn redo(&self) -> Result<bool>;

    /// Whether an undo is available.
    async fn can_undo(&self) -> bool;

    /// Whether a redo is available.
    async fn can_redo(&self) -> bool;
}
