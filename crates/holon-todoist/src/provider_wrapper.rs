//! OperationProvider wrapper for TodoistTaskOperations
//!
//! This wrapper implements OperationProvider for TodoistTaskOperations, enabling
//! generic property-based testing using GenericProviderState.

use crate::dispatch_helper::transform_undo_action;
use crate::models::TodoistTask;
use crate::todoist_datasource::TodoistTaskOperations;
use async_trait::async_trait;
use holon::core::datasource::{
    __operations_block_operations, __operations_crud_operations, __operations_task_operations,
    CrudOperations, OperationDescriptor, OperationProvider, OperationResult, Result,
    UnknownOperationError,
};
use holon::storage::types::StorageEntity;
use holon_api::EntityName;
use std::sync::Arc;
use tracing::error;

/// OperationProvider wrapper for TodoistTaskOperations
///
/// This wrapper enables TodoistTaskOperations to be used with GenericProviderState
/// for property-based testing. It delegates to the generated dispatch_operation
/// functions from the #[operations_trait] macro.
pub struct TodoistOperationProvider {
    datasource: Arc<TodoistTaskOperations>,
    /// Store the last created entity ID (for GenericProviderState to retrieve)
    last_created_id: Arc<std::sync::Mutex<Option<String>>>,
}

impl TodoistOperationProvider {
    /// Create a new TodoistOperationProvider wrapping the given datasource
    pub fn new(datasource: Arc<TodoistTaskOperations>) -> Self {
        Self {
            datasource,
            last_created_id: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Get a reference to the underlying datasource
    pub fn datasource(&self) -> &Arc<TodoistTaskOperations> {
        &self.datasource
    }

    /// Get the last created entity ID (for GenericProviderState)
    pub fn get_last_created_id(&self) -> Option<String> {
        self.last_created_id.lock().unwrap().take()
    }
}

#[async_trait]
impl OperationProvider for TodoistOperationProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        // Use resolver function to get operations with resolved enum_from annotations
        crate::todoist_datasource::all_operations_with_resolver(self.datasource.as_ref())
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        use tracing::{debug, info};

        info!(
            "[TodoistOperationProvider] execute_operation: entity={}, op={}, params={:?}",
            entity_name, op_name, params
        );

        // Validate entity name
        if entity_name != "todoist_task" {
            error!(
                "[TodoistOperationProvider] Entity name mismatch: expected 'todoist_task', got '{}'",
                entity_name
            );
            return Err(
                format!("Expected entity_name 'todoist_task', got '{}'", entity_name).into(),
            );
        }

        // Special handling for create operation - need to extract the returned ID
        // The dispatch_operation functions return Result<OperationResult>, so we
        // need to call create() directly to get the ID for state tracking
        if op_name == "create" {
            info!("[TodoistOperationProvider] Executing create operation");
            // Call create() directly to get the ID
            // Dereference Arc to get &TodoistTaskOperations, then call trait method
            let (id, inverse) = <TodoistTaskOperations as CrudOperations<TodoistTask>>::create(
                self.datasource.as_ref(),
                params,
            )
            .await?;
            // Store the ID for GenericProviderState to retrieve
            *self.last_created_id.lock().unwrap() = Some(id.clone());
            info!(
                "[TodoistOperationProvider] Create operation succeeded: id={}",
                id
            );
            return Ok(OperationResult::from_undo(transform_undo_action(
                inverse.undo,
                entity_name.as_str(),
            )));
        }

        // Try dispatching to each trait module in order
        // The first one that succeeds wins
        debug!("[TodoistOperationProvider] Trying CrudOperations dispatch");
        match __operations_crud_operations::dispatch_operation(
            self.datasource.as_ref(),
            op_name,
            &params,
        )
        .await
        {
            Ok(inverse) => {
                info!(
                    "[TodoistOperationProvider] Operation succeeded via CrudOperations: op={}",
                    op_name
                );
                return Ok(OperationResult::from_undo(transform_undo_action(
                    inverse.undo,
                    entity_name.as_str(),
                )));
            }
            Err(err) => {
                if !UnknownOperationError::is_unknown(err.as_ref()) {
                    error!(
                        "[TodoistOperationProvider] CrudOperations dispatch failed: op={}, error={}",
                        op_name, err
                    );
                    return Err(err);
                }
            }
        }

        debug!("[TodoistOperationProvider] Trying BlockOperations dispatch");
        match __operations_block_operations::dispatch_operation(
            self.datasource.as_ref(),
            op_name,
            &params,
        )
        .await
        {
            Ok(inverse) => {
                info!(
                    "[TodoistOperationProvider] Operation succeeded via BlockOperations: op={}",
                    op_name
                );
                return Ok(OperationResult::from_undo(transform_undo_action(
                    inverse.undo,
                    entity_name.as_str(),
                )));
            }
            Err(err) => {
                if !UnknownOperationError::is_unknown(err.as_ref()) {
                    error!(
                        "[TodoistOperationProvider] BlockOperations dispatch failed: op={}, error={}",
                        op_name, err
                    );
                    return Err(err);
                }
            }
        }

        debug!("[TodoistOperationProvider] Trying TaskOperations dispatch");
        let result = __operations_task_operations::dispatch_operation(
            self.datasource.as_ref(),
            op_name,
            &params,
        )
        .await;

        match &result {
            Ok(_) => {
                info!(
                    "[TodoistOperationProvider] Operation succeeded via TaskOperations: op={}",
                    op_name
                );
            }
            Err(e) => {
                error!(
                    "[TodoistOperationProvider] All dispatch attempts failed: op={}, error={}",
                    op_name, e
                );
            }
        }

        result.map(|inverse| {
            OperationResult::from_undo(transform_undo_action(inverse.undo, entity_name.as_str()))
        })
    }

    fn get_last_created_id(&self) -> Option<String> {
        // Call the struct method, not the trait method (to avoid infinite recursion)
        TodoistOperationProvider::get_last_created_id(self)
    }
}
