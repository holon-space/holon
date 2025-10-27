//! OperationDispatcher - Composite pattern implementation for operation routing
//!
//! The OperationDispatcher aggregates multiple OperationProvider instances and routes
//! operation execution to the correct provider based on entity_name.
//!
//! This implements the Composite Pattern - both individual caches (QueryableCache<T>)
//! and the dispatcher implement OperationProvider, allowing recursive composition.

use async_trait::async_trait;
use fluxdi::{Injector, Module, Provider, Shared};

use std::collections::HashSet;
use std::sync::Arc;
use tracing::{error, info};

use crate::core::datasource::{
    OperationObserver, OperationProvider, OperationResult, Result, SyncTokenStore, UndoAction,
};
use crate::storage::types::StorageEntity;
use holon_api::{EntityName, Operation, OperationDescriptor};

/// Composite dispatcher that aggregates multiple OperationProvider instances
///
/// Routes operations to the correct provider based on entity_name.
/// Implements OperationProvider itself, enabling recursive composition.
/// Supports wildcard entity_name "*" to execute operations on all matching providers.
///
/// Also supports OperationObservers that get notified after operations execute.
/// Observers can filter by entity_name or use "*" to observe all operations.
#[derive(Default)]
pub struct OperationDispatcher {
    providers: Vec<Arc<dyn OperationProvider>>,
    observers: Vec<Arc<dyn OperationObserver>>,
    sync_token_store: Option<Arc<dyn SyncTokenStore>>,
    matview_manager: Option<Arc<crate::sync::MatviewManager>>,
}

impl OperationDispatcher {
    pub fn new(providers: Vec<Arc<dyn OperationProvider>>) -> Self {
        Self {
            providers,
            ..Default::default()
        }
    }

    pub fn with_observers(
        providers: Vec<Arc<dyn OperationProvider>>,
        observers: Vec<Arc<dyn OperationObserver>>,
    ) -> Self {
        Self {
            providers,
            observers,
            ..Default::default()
        }
    }

    pub fn set_sync_token_store(&mut self, store: Arc<dyn SyncTokenStore>) {
        self.sync_token_store = Some(store);
    }

    pub fn set_matview_manager(&mut self, mgr: Arc<crate::sync::MatviewManager>) {
        self.matview_manager = Some(mgr);
    }

    /// Add an observer to this dispatcher
    pub fn add_observer(&mut self, observer: Arc<dyn OperationObserver>) {
        self.observers.push(observer);
    }

    /// Notify all matching observers of an executed operation
    async fn notify_observers(
        &self,
        entity_name: &str,
        operation: &Operation,
        undo_action: &UndoAction,
    ) {
        for observer in &self.observers {
            let filter = observer.entity_filter();
            if filter == "*" || filter == entity_name {
                observer.on_operation_executed(operation, undo_action).await;
            }
        }
    }

    /// Check if a provider is registered for an entity type
    pub fn has_provider(&self, entity_name: &str) -> bool {
        self.providers.iter().any(|provider| {
            provider
                .operations()
                .iter()
                .any(|op| op.entity_name == entity_name)
        })
    }

    /// Get list of registered entity names
    pub fn registered_entities(&self) -> Vec<EntityName> {
        let mut entity_names = HashSet::new();
        for provider in &self.providers {
            for op in provider.operations() {
                entity_names.insert(op.entity_name);
            }
        }
        entity_names.into_iter().collect()
    }

    /// Get the number of registered providers
    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }

    /// Get a copy of all providers (for reconstructing dispatcher with additional providers)
    pub fn providers(&self) -> Vec<Arc<dyn OperationProvider>> {
        self.providers.clone()
    }
}

#[async_trait]
impl OperationProvider for OperationDispatcher {
    /// Get all operations from all registered providers
    ///
    /// Aggregates operations from all providers and includes wildcard operations.
    fn operations(&self) -> Vec<OperationDescriptor> {
        let mut ops: Vec<OperationDescriptor> = self
            .providers
            .iter()
            .flat_map(|provider| provider.operations())
            .collect();

        // Add wildcard sync operation if any provider has a "sync" operation
        let has_sync_ops = ops.iter().any(|op| op.name == "sync");
        if has_sync_ops {
            ops.push(OperationDescriptor {
                entity_name: "*".into(),
                entity_short_name: "all".to_string(),
                id_column: String::new(),
                name: "sync".to_string(),
                display_name: "Sync".to_string(),
                description: "Sync registered syncable providers".to_string(),
                ..Default::default()
            });

            // Add wildcard full_sync operation (clear caches + sync)
            // This is triggered by Ctrl+clicking the sync button in the UI
            ops.push(OperationDescriptor {
                entity_name: "*".into(),
                entity_short_name: "all".to_string(),
                id_column: String::new(),
                name: "full_sync".to_string(),
                display_name: "Full Sync".to_string(),
                description:
                    "Clear all caches, reset sync tokens, and re-sync from external systems"
                        .to_string(),
                ..Default::default()
            });
        }

        ops
    }

    /// Find operations that can be executed with given arguments
    ///
    /// Filters operations based on entity_name and available_args.
    ///
    /// Special handling for generic operations:
    /// - `set_field`: Only requires "id" to be available (field and value are runtime parameters)
    /// - Other operations: Require all parameters to be in available_args
    fn find_operations(
        &self,
        entity_name: &EntityName,
        available_args: &[String],
    ) -> Vec<OperationDescriptor> {
        // Filter operations from all providers
        self.operations()
            .into_iter()
            .filter(|op| {
                if op.entity_name != *entity_name {
                    return false;
                }

                // Special case: set_field is a generic operation that can update any field
                // It only needs "id" from the query columns; "field" and "value" are runtime parameters
                if op.name == "set_field" {
                    // Only require "id" to be available
                    return op
                        .required_params
                        .iter()
                        .any(|p| p.name == "id" && available_args.contains(&p.name));
                }

                // For other operations, a param is considered available if:
                // 1. It's directly in available_args, OR
                // 2. It has a param_mapping that can provide it at runtime
                op.required_params.iter().all(|p| {
                    // Direct availability
                    if available_args.contains(&p.name) {
                        return true;
                    }
                    // Can be provided via param_mapping at runtime
                    op.param_mappings
                        .iter()
                        .any(|m| m.provides.contains(&p.name))
                })
            })
            .collect()
    }

    /// Execute an operation by routing to the correct provider
    ///
    /// # Arguments
    /// * `entity_name` - Entity identifier (e.g., "todoist-task" or "*" for wildcard)
    /// * `op_name` - Operation name (e.g., "set_state" or "sync")
    /// * `params` - Operation parameters as StorageEntity
    ///
    /// # Returns
    /// Result indicating success or failure
    ///
    /// # Errors
    /// Returns an error if:
    /// - No provider is registered for the entity_name (or wildcard matches no providers)
    /// - The provider's execute_operation returns an error
    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        use tracing::Instrument;
        use tracing::{debug, info};

        // Create tracing span that will be bridged to OpenTelemetry
        // Use .instrument() to maintain context across async boundaries
        let span = tracing::span!(
            tracing::Level::INFO,
            "dispatcher.execute_operation",
            "operation.entity" = entity_name.as_str(),
            "operation.name" = op_name
        );

        async {
            info!(
                "[OperationDispatcher] execute_operation: entity={}, op={}, params={:?}",
                entity_name, op_name, params
            );

            // Check if this is a wildcard operation
        if entity_name == "*" {
            info!(
                "[OperationDispatcher] Wildcard operation detected: op={}",
                op_name
            );

            // Special handling for full_sync: clear sync tokens, clear caches, then sync
            // IMPORTANT: Tokens must be cleared FIRST because clearing caches can trigger
            // sync_changes callbacks that would load and re-save the old token.
            if op_name == "full_sync" {
                info!("[OperationDispatcher] Executing full_sync: clearing sync tokens and caches first");

                // Step 1: Clear all sync tokens FIRST (so any triggered syncs start from Beginning)
                if let Some(ref token_store) = self.sync_token_store {
                    match token_store.clear_all_tokens().await {
                        Ok(_) => {
                            info!("[OperationDispatcher] Cleared all sync tokens");
                        }
                        Err(e) => {
                            error!("[OperationDispatcher] Failed to clear sync tokens: {}", e);
                        }
                    }
                } else {
                    info!("[OperationDispatcher] No sync token store configured, skipping token clearing");
                }

                // Step 2: Clear all caches (execute clear_cache on all providers that have it)
                for provider in &self.providers {
                    if let Some(op) = provider.operations().iter().find(|op| op.name == "clear_cache")
                    {
                        let entity_name = op.entity_name.as_str();
                        match provider
                            .execute_operation(&op.entity_name, "clear_cache", StorageEntity::new())
                            .await
                        {
                            Ok(_) => {
                                info!(
                                    "[OperationDispatcher] Cleared cache for entity '{}'",
                                    entity_name
                                );
                            }
                            Err(e) => {
                                error!(
                                    "[OperationDispatcher] Failed to clear cache for entity '{}': {}",
                                    entity_name, e
                                );
                            }
                        }
                    }
                }

                // Step 3: Drop stale matviews so they get recreated fresh
                if let Some(ref mgr) = self.matview_manager {
                    match mgr.drop_stale_views().await {
                        Ok(()) => info!("[OperationDispatcher] Dropped stale matviews"),
                        Err(e) => error!("[OperationDispatcher] Failed to drop stale matviews: {e}"),
                    }
                }

                // Step 4: Execute sync on all providers that have it
                info!("[OperationDispatcher] Executing sync on all providers");
                let mut sync_success_count = 0;
                let mut sync_error_count = 0;
                for provider in &self.providers {
                    if let Some(op) = provider.operations().iter().find(|op| op.name == "sync") {
                        let entity_name = op.entity_name.as_str();
                        match provider
                            .execute_operation(&op.entity_name, "sync", StorageEntity::new())
                            .await
                        {
                            Ok(_) => {
                                sync_success_count += 1;
                                info!(
                                    "[OperationDispatcher] Sync succeeded for entity '{}'",
                                    entity_name
                                );
                            }
                            Err(e) => {
                                sync_error_count += 1;
                                error!(
                                    "[OperationDispatcher] Sync failed for entity '{}': {}",
                                    entity_name, e
                                );
                            }
                        }
                    }
                }

                info!(
                    "[OperationDispatcher] full_sync completed: {} sync succeeded, {} failed",
                    sync_success_count, sync_error_count
                );
                return Ok(OperationResult::irreversible(Vec::new()));
            }

            // Find all providers that have an operation with matching op_name
            let mut matching_providers = Vec::new();
            for provider in &self.providers {
                let ops = provider.operations();
                if ops.iter().any(|op| op.name == op_name) {
                    matching_providers.push(provider.clone());
                }
            }

            if matching_providers.is_empty() {
                error!(
                    "[OperationDispatcher] No providers found with operation '{}' for wildcard dispatch",
                    op_name
                );
                return Err(format!(
                    "No providers found with operation '{}' for wildcard dispatch",
                    op_name
                )
                .into());
            }

            info!(
                "[OperationDispatcher] Found {} providers with operation '{}'",
                matching_providers.len(),
                op_name
            );

            // Execute operation on each matching provider
            let mut success_count = 0;
            let mut error_count = 0;
            for provider in matching_providers {
                // For wildcard operations, we need to find the actual entity_name from the provider
                // Find the first operation with matching op_name
                let ops = provider.operations();
                if let Some(op) = ops.iter().find(|op| op.name == op_name) {
                    let actual_entity_name = op.entity_name.as_str();
                    match provider
                        .execute_operation(&op.entity_name, op_name, params.clone())
                        .await
                    {
                        Ok(_) => {
                            success_count += 1;
                            info!(
                                "[OperationDispatcher] Wildcard operation succeeded on entity '{}'",
                                actual_entity_name
                            );
                        }
                        Err(e) => {
                            error_count += 1;
                            error!(
                                "[OperationDispatcher] Wildcard operation failed on entity '{}': {}",
                                actual_entity_name, e
                            );
                        }
                    }
                }
            }

            // Return success if at least one provider succeeded
            // For wildcard operations, we can't return a single inverse operation
            // since multiple providers might have executed
            if success_count > 0 {
                info!(
                    "[OperationDispatcher] Wildcard operation completed: {} succeeded, {} failed",
                    success_count, error_count
                );
                Ok(OperationResult::irreversible(Vec::new())) // Wildcard operations can't be undone as a single operation
            } else {
                error!(
                    "[OperationDispatcher] Wildcard operation failed on all {} providers",
                    error_count
                );
                Err(format!(
                    "Wildcard operation '{}' failed on all {} providers",
                    op_name, error_count
                )
                .into())
            }
        } else {
            // Regular operation - route to specific provider
            let available_ops: Vec<_> = self.providers.iter().flat_map(|p| p.operations()).collect();
            let entity_name_str = entity_name.as_str();
            let matching_ops: Vec<_> = available_ops
                .iter()
                .filter(|op| op.entity_name == entity_name_str && op.name == op_name)
                .collect();

            debug!(
                "[OperationDispatcher] Found {} matching operations for entity={}, op={}",
                matching_ops.len(), entity_name, op_name
            );

            // If no direct match, try inferring entity type from the `id` param's
            // URI scheme. Rows from matviews/views carry the view name as entity_name
            // (e.g. "focus_roots") but the actual entity provider is registered under
            // the scheme (e.g. "block" from "block:xxx").
            let resolved_entity: String;
            let resolved_entity_name: &str = if matching_ops.is_empty() {
                let scheme = params
                    .get("id")
                    .and_then(|v| match v {
                        holon_api::Value::String(s) => s.split_once(':').map(|(scheme, _)| scheme.to_string()),
                        _ => None,
                    });

                if let Some(scheme) = scheme {
                    let has_match = available_ops
                        .iter()
                        .any(|op| op.entity_name == scheme.as_str() && op.name == op_name);
                    if has_match {
                        info!(
                            "[OperationDispatcher] Entity '{}' not found, resolved to '{}' via id scheme",
                            entity_name, scheme
                        );
                        resolved_entity = scheme;
                        resolved_entity.as_str()
                    } else {
                        entity_name_str
                    }
                } else {
                    entity_name_str
                }
            } else {
                entity_name_str
            };

            if !available_ops.iter().any(|op| op.entity_name == resolved_entity_name && op.name == op_name) {
                let entity_names: std::collections::HashSet<_> =
                    available_ops.iter().map(|op| &op.entity_name).collect();
                error!(
                    "[OperationDispatcher] No provider registered for entity: '{}' (operation: '{}'). Available entities: {:?}",
                    entity_name, op_name, entity_names
                );
                return Err(format!("No provider registered for entity: {}", entity_name).into());
            }

            let provider = self
                .providers
                .iter()
                .find(|provider| {
                    provider
                        .operations()
                        .iter()
                        .any(|op| op.entity_name == resolved_entity_name && op.name == op_name)
                })
                .ok_or_else(|| format!("No provider registered for entity: {}", entity_name))?;

            info!(
                "[OperationDispatcher] Routing operation to provider: entity={}, op={}",
                resolved_entity_name, op_name
            );

            // Clone params before execution for observer notification
            let params_for_observer = params.clone();
            let resolved_entity_name_typed = EntityName::new(resolved_entity_name);

            // Execute operation and get result with changes and undo action
            let mut operation_result = provider
                .execute_operation(&resolved_entity_name_typed, op_name, params)
                .await?;
            // Set entity_name on the inverse operation if present
            operation_result.undo = match operation_result.undo {
                UndoAction::Undo(mut op) => {
                    op.entity_name = resolved_entity_name_typed.clone();
                    UndoAction::Undo(op)
                }
                UndoAction::Irreversible => UndoAction::Irreversible,
            };

            match &operation_result.undo {
                UndoAction::Undo(_) => {
                    info!(
                        "[OperationDispatcher] Provider execution succeeded: entity={}, op={} (inverse operation available)",
                        entity_name, op_name
                    );
                }
                UndoAction::Irreversible => {
                    info!(
                        "[OperationDispatcher] Provider execution succeeded: entity={}, op={} (no inverse operation)",
                        entity_name, op_name
                    );
                }
            }

            // Notify observers of successful execution
            let executed_operation = Operation::new(resolved_entity_name, op_name, "", params_for_observer);
            self.notify_observers(resolved_entity_name, &executed_operation, &operation_result.undo).await;

            // Execute follow-up operations (e.g., editor_focus after split_block).
            for follow_up in std::mem::take(&mut operation_result.follow_ups) {
                let fu_entity = follow_up.entity_name.clone();
                let fu_op = follow_up.op_name.clone();
                info!(
                    "[OperationDispatcher] Executing follow-up: entity={}, op={}",
                    fu_entity, fu_op
                );
                self.execute_operation(&fu_entity, &fu_op, follow_up.params)
                    .await
                    .map_err(|e| format!("Follow-up {fu_entity}.{fu_op} failed: {e}"))?;
            }

            Ok(operation_result)
        }
        }
        .instrument(span)
        .await
    }
}

pub struct OperationModule;

impl Module for OperationModule {
    fn configure(&self, injector: &Injector) -> std::result::Result<(), fluxdi::Error> {
        injector.provide::<OperationDispatcher>(Provider::root_async(|r| async move {
            let providers = r
                .try_resolve_all_async::<dyn OperationProvider>()
                .await
                .expect("Failed to get all operation providers");
            info!(
                "[OperationModule] Found {} operation providers",
                providers.len()
            );
            let observers = r
                .try_resolve_all_async::<dyn OperationObserver>()
                .await
                .unwrap_or_else(|_| vec![]);
            info!(
                "[OperationModule] Found {} operation observers",
                observers.len()
            );

            let sync_token_store = r.optional_resolve_async::<dyn SyncTokenStore>().await;
            if sync_token_store.is_some() {
                info!("[OperationModule] SyncTokenStore configured for full_sync support");
            }

            let db_handle_provider = r.resolve::<dyn crate::di::DbHandleProvider>();
            let ddl_mutex = std::sync::Arc::new(tokio::sync::Mutex::new(()));
            let matview_mgr = Arc::new(crate::sync::MatviewManager::new(
                db_handle_provider.handle(),
                ddl_mutex,
            ));
            let mut dispatcher = OperationDispatcher::with_observers(providers, observers);
            if let Some(store) = sync_token_store {
                dispatcher.set_sync_token_store(store);
            }
            dispatcher.set_matview_manager(matview_mgr);
            Shared::new(dispatcher)
        }));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use self::super::*;

    // Mock OperationProvider for testing
    struct MockProvider {
        entity_name: String,
        operations_list: Vec<OperationDescriptor>,
    }

    #[async_trait]
    impl OperationProvider for MockProvider {
        fn operations(&self) -> Vec<OperationDescriptor> {
            self.operations_list.clone()
        }

        async fn execute_operation(
            &self,
            entity_name: &EntityName,
            op_name: &str,
            _params: StorageEntity,
        ) -> Result<OperationResult> {
            if entity_name != self.entity_name.as_str() {
                return Err(format!(
                    "Entity mismatch: expected {}, got {}",
                    self.entity_name, entity_name
                )
                .into());
            }
            if op_name == "test_op" {
                Ok(OperationResult::irreversible(Vec::new()))
            } else {
                Err(format!("Unknown operation: {}", op_name).into())
            }
        }
    }

    fn create_test_operation(entity_name: &str, op_name: &str) -> OperationDescriptor {
        OperationDescriptor {
            entity_name: entity_name.into(),
            entity_short_name: entity_name.to_string(),
            id_column: "id".to_string(),
            name: op_name.to_string(),
            display_name: format!("Test {}", op_name),
            description: format!("Test operation {}", op_name),
            required_params: vec![],
            affected_fields: vec![],
            param_mappings: vec![],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_provider_registration() {
        let provider1 = Arc::new(MockProvider {
            entity_name: "entity1".to_string(),
            operations_list: vec![create_test_operation("entity1", "op1")],
        });

        let dispatcher = OperationDispatcher::new(vec![provider1]);
        assert!(dispatcher.has_provider("entity1"));
        assert_eq!(dispatcher.provider_count(), 1);
    }

    #[tokio::test]
    async fn test_operations_aggregation() {
        let provider1 = Arc::new(MockProvider {
            entity_name: "entity1".to_string(),
            operations_list: vec![
                create_test_operation("entity1", "op1"),
                create_test_operation("entity1", "op2"),
            ],
        });

        let provider2 = Arc::new(MockProvider {
            entity_name: "entity2".to_string(),
            operations_list: vec![create_test_operation("entity2", "op3")],
        });

        let dispatcher = OperationDispatcher::new(vec![provider1, provider2]);

        let all_ops = dispatcher.operations();
        assert_eq!(all_ops.len(), 3);
        assert!(all_ops.iter().any(|op| op.name == "op1"));
        assert!(all_ops.iter().any(|op| op.name == "op2"));
        assert!(all_ops.iter().any(|op| op.name == "op3"));
    }

    #[tokio::test]
    async fn test_execute_operation_routing() {
        let provider1 = Arc::new(MockProvider {
            entity_name: "entity1".to_string(),
            operations_list: vec![create_test_operation("entity1", "test_op")],
        });

        let dispatcher = OperationDispatcher::new(vec![provider1]);

        // Execute operation on registered entity
        let params = StorageEntity::new();
        let result = dispatcher
            .execute_operation(&EntityName::new("entity1"), "test_op", params)
            .await;
        assert!(result.is_ok());

        // Try to execute on unregistered entity
        let params = StorageEntity::new();
        let result = dispatcher
            .execute_operation(&EntityName::new("entity2"), "test_op", params)
            .await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No provider registered")
        );
    }

    #[tokio::test]
    async fn test_registered_entities() {
        let provider1 = Arc::new(MockProvider {
            entity_name: "entity1".to_string(),
            operations_list: vec![create_test_operation("entity1", "op1")],
        });
        let provider2 = Arc::new(MockProvider {
            entity_name: "entity2".to_string(),
            operations_list: vec![create_test_operation("entity2", "op2")],
        });

        let dispatcher = OperationDispatcher::new(vec![provider1, provider2]);

        let entities = dispatcher.registered_entities();
        assert_eq!(entities.len(), 2);
        assert!(entities.contains(&EntityName::new("entity1")));
        assert!(entities.contains(&EntityName::new("entity2")));
    }
}
