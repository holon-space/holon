//! The operation-execution capability (ADR 0004 — "Turso is one of four").
//!
//! `OperationEngine` is the seam the frontend's mutation/operation path depends
//! on instead of the concrete Turso [`BackendEngine`](crate::api::BackendEngine).
//! It covers dispatching operations, discovering which operations an entity
//! supports, and undo/redo. Operations are *not* fundamentally Turso-bound — they
//! flow through the [`OperationDispatcher`](crate::api::OperationDispatcher) and a
//! per-session undo stack — so a future no-Turso wiring can provide this
//! capability over the Loro consolidator. Today only `BackendEngine` implements
//! it; the frontend holds it as `Option<Arc<dyn OperationEngine>>` so a no-Turso
//! session reports the capability's absence as a typed fact rather than panicking
//! behind `engine()`.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use holon_api::{EntityName, Operation, OperationDescriptor, Value};
use holon_core::{UndoAction, UndoStack};
use tokio::sync::RwLock;

use crate::api::BackendEngine;
use crate::api::operation_dispatcher::OperationDispatcher;
use crate::core::datasource::OperationProvider;
use crate::storage::types::StorageEntity;

pub use holon_api::operation_engine::OperationEngine;

#[async_trait]
impl OperationEngine for BackendEngine {
    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<Option<Value>> {
        BackendEngine::execute_operation(self, entity_name, op_name, params).await
    }

    async fn available_operations(&self, entity_name: &str) -> Vec<OperationDescriptor> {
        BackendEngine::available_operations(self, entity_name).await
    }

    async fn has_operation(&self, entity_name: &str, op_name: &str) -> bool {
        BackendEngine::has_operation(self, entity_name, op_name).await
    }

    async fn undo(&self) -> Result<bool> {
        BackendEngine::undo(self).await
    }

    async fn redo(&self) -> Result<bool> {
        BackendEngine::redo(self).await
    }

    async fn can_undo(&self) -> bool {
        BackendEngine::can_undo(self).await
    }

    async fn can_redo(&self) -> bool {
        BackendEngine::can_redo(self).await
    }
}

/// A backend-agnostic [`OperationEngine`] over a bare
/// [`OperationDispatcher`](crate::api::OperationDispatcher) plus a per-session
/// [`UndoStack`]. This is the operation capability for a no-Turso (Loro-only)
/// session: it carries the same dispatch + undo/redo logic as the Turso
/// [`BackendEngine`] but without any of Turso's query/CDC machinery, so a
/// session that registers Loro-native operation providers (e.g.
/// `LoroBlockOperations`) gets full mutation + undo support.
pub struct DispatchingOperationEngine {
    dispatcher: Arc<OperationDispatcher>,
    undo_stack: Arc<RwLock<UndoStack>>,
}

impl DispatchingOperationEngine {
    /// Build an engine over the given dispatcher. The undo stack starts empty.
    pub fn new(dispatcher: Arc<OperationDispatcher>) -> Self {
        Self {
            dispatcher,
            undo_stack: Arc::new(RwLock::new(UndoStack::default())),
        }
    }
}

#[async_trait]
impl OperationEngine for DispatchingOperationEngine {
    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<Option<Value>> {
        let original_op = Operation::new(
            entity_name.clone(),
            op_name,
            "",
            params
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        );

        let result = self
            .dispatcher
            .execute_operation(entity_name, op_name, params)
            .await
            .map_err(|e| {
                anyhow::anyhow!("Operation '{op_name}' on entity '{entity_name}' failed: {e}")
            })?;

        if let UndoAction::Undo(inverse_op) = &result.undo {
            self.undo_stack
                .write()
                .await
                .push(original_op, inverse_op.clone());
        }

        Ok(result.response)
    }

    async fn available_operations(&self, entity_name: &str) -> Vec<OperationDescriptor> {
        self.dispatcher
            .operations()
            .into_iter()
            .filter(|op| op.entity_name == entity_name)
            .collect()
    }

    async fn has_operation(&self, entity_name: &str, op_name: &str) -> bool {
        self.dispatcher
            .operations()
            .into_iter()
            .any(|op| op.entity_name == entity_name && op.name == op_name)
    }

    async fn undo(&self) -> Result<bool> {
        let inverse_op = {
            let mut stack = self.undo_stack.write().await;
            match stack.pop_for_undo() {
                Some(op) => op,
                None => return Ok(false),
            }
        };

        let result = self
            .dispatcher
            .execute_operation(
                &inverse_op.entity_name,
                &inverse_op.op_name,
                inverse_op
                    .params
                    .iter()
                    .map(|(k, v)| (Arc::from(k.as_str()), v.clone()))
                    .collect(),
            )
            .await
            .map_err(|e| anyhow::anyhow!("Failed to execute undo operation: {e}"))?;

        if let UndoAction::Undo(new_inverse_op) = result.undo {
            self.undo_stack
                .write()
                .await
                .update_redo_top(new_inverse_op);
        }

        Ok(true)
    }

    async fn redo(&self) -> Result<bool> {
        let operation_to_redo = {
            let mut stack = self.undo_stack.write().await;
            match stack.pop_for_redo() {
                Some(op) => op,
                None => return Ok(false),
            }
        };

        let result = self
            .dispatcher
            .execute_operation(
                &operation_to_redo.entity_name,
                &operation_to_redo.op_name,
                operation_to_redo
                    .params
                    .iter()
                    .map(|(k, v)| (Arc::from(k.as_str()), v.clone()))
                    .collect(),
            )
            .await
            .map_err(|e| anyhow::anyhow!("Failed to execute redo operation: {e}"))?;

        if let UndoAction::Undo(new_inverse_op) = result.undo {
            self.undo_stack
                .write()
                .await
                .update_undo_top(new_inverse_op);
        }

        Ok(true)
    }

    async fn can_undo(&self) -> bool {
        self.undo_stack.read().await.can_undo()
    }

    async fn can_redo(&self) -> bool {
        self.undo_stack.read().await.can_redo()
    }
}
