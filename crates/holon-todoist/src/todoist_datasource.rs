//! Real Todoist datasource implementation for stream-based architecture
//!
//! TodoistTaskOperations implements CrudOperations<TodoistTask> and TaskOperations<TodoistTask>:
//! - Uses QueryableCache for fast lookups (data populated via change streams)
//! - Makes HTTP calls to Todoist API for mutations only
//! - Returns immediately (fire-and-forget)
//! - Changes arrive via TodoistSyncProvider stream into the cache

use async_trait::async_trait;
use holon::core::datasource::{
    __operations_block_operations, __operations_task_operations, BlockDataSourceHelpers,
    BlockMaintenanceHelpers, BlockOperations, BlockQueryHelpers, CompletionStateInfo,
    CrudOperations, DataSource, OperationDescriptor, OperationProvider, OperationRegistry,
    OperationResult, Result, TaskOperations, UndoAction, UnknownOperationError,
};
use holon::core::queryable_cache::QueryableCache;
use holon::storage::types::StorageEntity;
use holon_api::streaming::ChangeNotifications;
use holon_api::{ApiError, Change, EntityName, StreamPosition};
use holon_api::{OperationParam, ParamMapping, TypeHint, Value};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use crate::models::{TodoistProject, TodoistProjectApiResponse, TodoistTask};

use super::todoist_sync_provider::TodoistSyncProvider;
use tokio::sync::broadcast;
use tokio_stream::Stream;
use tracing::{debug, error, info};

/// Todoist-specific move operations that use entity-typed parameters
///
/// These operations are triggered by entity-typed params (e.g., `project_id`, `task_id`)
/// rather than generic params like `parent_id`. This allows automatic operation matching
/// based on the drop target's entity type.
#[holon_macros::operations_trait]
#[async_trait]
pub trait TodoistMoveOperations: Send + Sync {
    /// Move a task to a project (at root level, not under another task)
    #[holon_macros::affects("project_id", "parent_id")]
    #[holon_macros::triggered_by(availability_of = "project_id")]
    async fn move_to_project(&self, id: &str, project_id: &str) -> Result<OperationResult>;

    /// Move a task under another task (as a subtask)
    #[holon_macros::affects("parent_id")]
    #[holon_macros::triggered_by(availability_of = "task_id")]
    async fn move_under_task(&self, id: &str, task_id: &str) -> Result<OperationResult>;
}

/// Operations for TodoistTask
///
/// This struct uses a QueryableCache for lookups and TodoistSyncProvider for API mutations.
/// Data flows into the cache via change streams; this struct only performs lookups and mutations.
pub struct TodoistTaskOperations {
    cache: Arc<QueryableCache<TodoistTask>>,
    provider: Arc<TodoistSyncProvider>,
}

impl TodoistTaskOperations {
    pub fn new(
        cache: Arc<QueryableCache<TodoistTask>>,
        provider: Arc<TodoistSyncProvider>,
    ) -> Self {
        Self { cache, provider }
    }
}

#[async_trait]
impl TodoistMoveOperations for TodoistTaskOperations {
    async fn move_to_project(&self, id: &str, project_id: &str) -> Result<OperationResult> {
        info!(
            "[TodoistTaskOperations] move_to_project: task {} -> project {}",
            id, project_id
        );

        // Capture old state for inverse operation
        let old_task = self
            .cache
            .get_by_id(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Task not found"))?;
        let old_project_id = old_task.project_id.clone();
        let old_parent_id = old_task.parent_id.clone();

        self.provider
            .client
            .move_task(id, None, Some(project_id), None)
            .await?;

        if let Some(old_parent_id) = old_parent_id {
            Ok(OperationResult::from_undo(UndoAction::Undo(
                __operations_todoist_move_operations::move_under_task_op(
                    "", // Will be set by OperationProvider
                    id,
                    &old_parent_id,
                ),
            )))
        } else {
            Ok(OperationResult::from_undo(UndoAction::Undo(
                __operations_todoist_move_operations::move_to_project_op(
                    "", // Will be set by OperationProvider
                    id,
                    &old_project_id,
                ),
            )))
        }
    }

    async fn move_under_task(&self, id: &str, task_id: &str) -> Result<OperationResult> {
        info!(
            "[TodoistTaskOperations] move_under_task: task {} -> parent task {}",
            id, task_id
        );

        // Capture old state for inverse operation
        let old_task = self
            .cache
            .get_by_id(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Task not found"))?;
        let old_parent_id = old_task.parent_id.clone();
        let old_project_id = old_task.project_id.clone();

        self.provider
            .client
            .move_task(id, Some(task_id), None, None)
            .await?;

        // Return inverse operation - restore old parent or move to project
        // The macro generates __operations_todoist_move_operations module (in same file)
        if let Some(old_parent) = &old_parent_id {
            Ok(OperationResult::from_undo(UndoAction::Undo(
                __operations_todoist_move_operations::move_under_task_op(
                    "", // Will be set by OperationProvider
                    id, old_parent,
                ),
            )))
        } else {
            // Was at root level, restore to project
            Ok(OperationResult::from_undo(UndoAction::Undo(
                __operations_todoist_move_operations::move_to_project_op(
                    "", // Will be set by OperationProvider
                    id,
                    &old_project_id,
                ),
            )))
        }
    }
}

#[async_trait]
impl TaskOperations<TodoistTask> for TodoistTaskOperations {
    // Handled by MCP update_tasks — trait impl required for type bounds only
    async fn set_title(&self, _id: &str, _content: &str) -> Result<OperationResult> {
        unreachable!("set_title is handled by MCP update_tasks")
    }

    fn completion_states_with_progress(&self) -> Vec<CompletionStateInfo> {
        vec![
            CompletionStateInfo {
                state: "active".into(),
                progress: 0.0,
                is_done: false,
                is_active: true,
            },
            CompletionStateInfo {
                state: "completed".into(),
                progress: 100.0,
                is_done: true,
                is_active: false,
            },
        ]
    }

    async fn set_state(&self, id: &str, task_state: String) -> Result<OperationResult> {
        info!(
            "[TodoistTaskOperations] set_state: id={}, state={}",
            id, task_state
        );

        let old_task = self
            .cache
            .get_by_id(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Task not found"))?;
        let old_state = if old_task.completed {
            "completed"
        } else {
            "active"
        }
        .to_string();

        let completed = task_state == "completed";

        let result = if completed {
            debug!("[TodoistTaskOperations] Closing task");
            self.provider.client.close_task(id).await
        } else {
            debug!("[TodoistTaskOperations] Reopening task");
            self.provider.client.reopen_task(id).await
        };

        match &result {
            Ok(_) => {
                info!(
                    "[TodoistTaskOperations] set_state succeeded: id={}, task_state={}",
                    id, task_state
                );
            }
            Err(e) => {
                error!(
                    "[TodoistTaskOperations] set_state failed: id={}, task_state={}, error={}",
                    id, task_state, e
                );
            }
        }

        result.map(|_| {
            use holon::core::datasource::__operations_task_operations;
            OperationResult::from_undo(UndoAction::Undo(
                __operations_task_operations::set_state_op(
                    "", // Will be set by OperationProvider
                    id, old_state,
                ),
            ))
        })
    }

    async fn cycle_task_state(&self, id: &str) -> Result<OperationResult> {
        let task = self
            .cache
            .get_by_id(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Task not found: {id}"))?;
        let next = if task.completed {
            "active".to_string()
        } else {
            "completed".to_string()
        };
        self.set_state(id, next).await
    }

    // Handled by MCP update_tasks — trait impl required for type bounds only
    async fn set_priority(&self, _id: &str, _priority: i64) -> Result<OperationResult> {
        unreachable!("set_priority is handled by MCP update_tasks")
    }

    // Handled by MCP update_tasks — trait impl required for type bounds only
    async fn set_due_date(
        &self,
        _id: &str,
        _due_date: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<OperationResult> {
        unreachable!("set_due_date is handled by MCP update_tasks")
    }
}

#[async_trait]
impl ChangeNotifications<TodoistTask> for TodoistTaskOperations {
    async fn watch_changes_since(
        &self,
        _position: StreamPosition,
    ) -> Pin<Box<dyn Stream<Item = std::result::Result<Vec<Change<TodoistTask>>, ApiError>> + Send>>
    {
        let rx = self.provider.subscribe_tasks();

        // Convert broadcast receiver to stream, extracting inner changes from metadata wrapper
        // Note: The sync token in metadata is handled by QueryableCache.ingest_stream_with_metadata()
        let change_stream = futures::stream::unfold(rx, |mut rx| async move {
            match rx.recv().await {
                Ok(batch_with_metadata) => {
                    // Extract inner changes from metadata wrapper
                    Some((Ok(batch_with_metadata.inner), rx))
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::debug!("Stream lagged by {} messages", n);
                    Some((
                        Err(ApiError::InternalError {
                            message: format!("Stream lagged by {} messages", n),
                        }),
                        rx,
                    ))
                }
                Err(broadcast::error::RecvError::Closed) => None,
            }
        });

        Box::pin(change_stream)
    }

    async fn get_current_version(&self) -> std::result::Result<Vec<u8>, ApiError> {
        // Note: Sync tokens are now managed externally (by OperationDispatcher or caller)
        // This method should return the current version from the dispatcher or database
        // For now, return empty vec - the version should be retrieved from OperationDispatcher
        // TODO: Get sync token from OperationDispatcher or database
        Ok(Vec::new())
    }
}

// DataSource implementation delegates to the cache for fast lookups
#[async_trait]
impl holon::core::datasource::DataSource<TodoistTask> for TodoistTaskOperations {
    async fn get_all(&self) -> Result<Vec<TodoistTask>> {
        self.cache.get_all().await
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<TodoistTask>> {
        self.cache.get_by_id(id).await
    }
}

impl BlockQueryHelpers<TodoistTask> for TodoistTaskOperations {}
impl BlockMaintenanceHelpers<TodoistTask> for TodoistTaskOperations {}
impl BlockDataSourceHelpers<TodoistTask> for TodoistTaskOperations {}
impl BlockOperations<TodoistTask> for TodoistTaskOperations {}

// CrudOperations trait impl required for type bounds in dispatch functions.
// All methods are now handled by MCP (update_tasks, add_tasks, delete_object).
#[async_trait]
impl CrudOperations<TodoistTask> for TodoistTaskOperations {
    async fn set_field(&self, _id: &str, _field: &str, _value: Value) -> Result<OperationResult> {
        unreachable!("set_field is handled by MCP update_tasks")
    }

    async fn create(&self, _fields: HashMap<String, Value>) -> Result<(String, OperationResult)> {
        unreachable!("create is handled by MCP add_tasks")
    }

    async fn delete(&self, _id: &str) -> Result<OperationResult> {
        unreachable!("delete is handled by MCP delete_object")
    }
}

/// Operations now handled by MCP (via McpOperationProvider + sidecar undo).
/// Excluded from hand-written operation descriptors to avoid duplicates.
const MCP_HANDLED_OPS: &[&str] = &[
    "set_field",
    "create",
    "delete",
    "set_title",
    "set_priority",
    "set_due_date",
];

/// Get ALL operations for TodoistTask including CRUD operations.
/// Used by the fake/test wrappers where MCP is not available.
pub fn all_operations_with_resolver<DS>(ds: &DS) -> Vec<OperationDescriptor>
where
    DS: holon::core::datasource::TaskOperations<TodoistTask> + Send + Sync,
{
    let entity_name = <TodoistTask as OperationRegistry>::entity_name();
    let short_name =
        <TodoistTask as OperationRegistry>::short_name().expect("TodoistTask must have short_name");
    let table = entity_name;
    let id_column = "id";

    use holon::core::datasource::{
        __operations_block_operations, __operations_crud_operations, __operations_task_operations,
    };

    let mut ops = __operations_task_operations::task_operations_with_resolver(
        ds,
        entity_name,
        short_name,
        table,
        id_column,
    );
    ops.extend(
        __operations_crud_operations::crud_operations(entity_name, short_name, table, id_column)
            .into_iter(),
    );
    ops.extend(
        __operations_block_operations::block_operations(entity_name, short_name, table, id_column)
            .into_iter(),
    );
    ops.extend(
        __operations_todoist_move_operations::todoist_move_operations(
            entity_name,
            short_name,
            table,
            id_column,
        )
        .into_iter(),
    );
    ops
}

/// Get operations for TodoistTask, excluding those handled by MCP.
///
/// Operations handled by MCP (set_field, create, delete, set_title, set_priority, set_due_date)
/// are excluded — the MCP provider advertises those via its sidecar instead.
pub fn operations_with_resolver<DS>(ds: &DS) -> Vec<OperationDescriptor>
where
    DS: holon::core::datasource::TaskOperations<TodoistTask> + Send + Sync,
{
    let entity_name = <TodoistTask as OperationRegistry>::entity_name();
    let short_name =
        <TodoistTask as OperationRegistry>::short_name().expect("TodoistTask must have short_name");
    let table = entity_name;
    let id_column = "id";

    use holon::core::datasource::__operations_task_operations;

    // Only keep set_state from task operations (the rest are handled by MCP)
    let mut ops: Vec<OperationDescriptor> =
        __operations_task_operations::task_operations_with_resolver(
            ds,
            entity_name,
            short_name,
            table,
            id_column,
        )
        .into_iter()
        .filter(|op| !MCP_HANDLED_OPS.contains(&op.name.as_str()))
        .collect();

    // Block operations (move_block, indent, outdent) — no MCP equivalent
    use holon::core::datasource::__operations_block_operations;
    ops.extend(
        __operations_block_operations::block_operations(entity_name, short_name, table, id_column)
            .into_iter(),
    );

    // Todoist-specific move operations — no MCP equivalent
    ops.extend(
        __operations_todoist_move_operations::todoist_move_operations(
            entity_name,
            short_name,
            table,
            id_column,
        )
        .into_iter(),
    );

    ops
}

/// OperationProvider implementation for TodoistTaskOperations
#[async_trait]
impl OperationProvider for TodoistTaskOperations {
    fn operations(&self) -> Vec<OperationDescriptor> {
        operations_with_resolver(self)
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        // Validate entity name
        if entity_name != "todoist_task" {
            return Err(
                format!("Expected entity_name 'todoist_task', got '{}'", entity_name).into(),
            );
        }

        // Todoist-specific move operations (move_to_project, move_under_task)
        match __operations_todoist_move_operations::dispatch_operation(self, op_name, &params).await
        {
            Ok(result) => {
                return Ok(OperationResult::from_undo(match result.undo {
                    UndoAction::Undo(mut op) => {
                        op.entity_name = entity_name.clone();
                        UndoAction::Undo(op)
                    }
                    UndoAction::Irreversible => UndoAction::Irreversible,
                }));
            }
            Err(err) => {
                if !UnknownOperationError::is_unknown(err.as_ref()) {
                    return Err(err);
                }
            }
        }

        // Block operations (move_block, indent, outdent)
        match __operations_block_operations::dispatch_operation::<_, TodoistTask>(
            self, op_name, &params,
        )
        .await
        {
            Ok(result) => {
                return Ok(OperationResult::from_undo(match result.undo {
                    UndoAction::Undo(mut op) => {
                        op.entity_name = entity_name.clone();
                        UndoAction::Undo(op)
                    }
                    UndoAction::Irreversible => UndoAction::Irreversible,
                }));
            }
            Err(err) => {
                if !UnknownOperationError::is_unknown(err.as_ref()) {
                    return Err(err);
                }
            }
        }

        // Task operations (set_state only — set_priority/set_due_date/set_title handled by MCP)
        let result = __operations_task_operations::dispatch_operation::<_, TodoistTask>(
            self, op_name, &params,
        )
        .await?;
        Ok(OperationResult::from_undo(match result.undo {
            UndoAction::Undo(mut op) => {
                op.entity_name = entity_name.clone();
                UndoAction::Undo(op)
            }
            UndoAction::Irreversible => UndoAction::Irreversible,
        }))
    }
}

/// DataSource implementation for TodoistProject
///
/// This wraps TodoistSyncProvider and implements ChangeNotifications<TodoistProject>.
/// Changes come from the sync provider's stream.
pub struct TodoistProjectDataSource {
    provider: Arc<TodoistSyncProvider>,
}

impl TodoistProjectDataSource {
    pub fn new(provider: Arc<TodoistSyncProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl ChangeNotifications<TodoistProject> for TodoistProjectDataSource {
    async fn watch_changes_since(
        &self,
        _position: StreamPosition,
    ) -> Pin<
        Box<dyn Stream<Item = std::result::Result<Vec<Change<TodoistProject>>, ApiError>> + Send>,
    > {
        let rx = self.provider.subscribe_projects();

        // Convert broadcast receiver to stream, extracting inner changes from metadata wrapper
        // Note: The sync token in metadata is handled by QueryableCache.ingest_stream_with_metadata()
        let change_stream = futures::stream::unfold(rx, |mut rx| async move {
            match rx.recv().await {
                Ok(batch_with_metadata) => {
                    // Extract inner changes from metadata wrapper
                    Some((Ok(batch_with_metadata.inner), rx))
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::debug!("Stream lagged by {} messages", n);
                    Some((
                        Err(ApiError::InternalError {
                            message: format!("Stream lagged by {} messages", n),
                        }),
                        rx,
                    ))
                }
                Err(broadcast::error::RecvError::Closed) => None,
            }
        });

        Box::pin(change_stream)
    }

    async fn get_current_version(&self) -> std::result::Result<Vec<u8>, ApiError> {
        // Note: Sync tokens are now managed externally (by OperationDispatcher or caller)
        // This method should return the current version from the dispatcher or database
        // For now, return empty vec - the version should be retrieved from OperationDispatcher
        // TODO: Get sync token from OperationDispatcher or database
        Ok(Vec::new())
    }
}

// Keep DataSource implementation for backward compatibility during migration
#[async_trait]
impl holon::core::datasource::DataSource<TodoistProject> for TodoistProjectDataSource {
    async fn get_all(&self) -> Result<Vec<TodoistProject>> {
        let sync_resp = self.provider.client.sync_projects(None).await?;

        // Extract projects from response
        let projects_array = sync_resp
            .get("projects")
            .and_then(|p| p.as_array())
            .ok_or_else(|| "No projects array in response".to_string())?;

        // Parse projects
        let projects: Vec<TodoistProject> = projects_array
            .iter()
            // ALLOW(filter_map_ok): TODO — should propagate, not silently drop
            .filter_map(|p| {
                serde_json::from_value::<TodoistProjectApiResponse>(p.clone())
                    .ok() // ALLOW(ok): TODO — should propagate, not silently drop
                    .filter(|api: &TodoistProjectApiResponse| !api.is_deleted.unwrap_or(false))
                    .map(|api| TodoistProject::from(api))
            })
            .collect();

        // Update sync token
        let _sync_token = sync_resp
            .get("sync_token")
            .and_then(|t| t.as_str())
            .map(|s| s.to_string());

        // Note: We can't update the provider's sync_token directly since it's private.
        // The sync provider manages its own token via sync() calls.
        // This is fine - the token will be updated when sync() is called.

        Ok(projects)
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<TodoistProject>> {
        // For projects, we need to sync to get a specific project
        // Since there's no direct "get project by ID" endpoint, we sync all projects
        let all_projects = self.get_all().await?;
        Ok(all_projects.into_iter().find(|p| p.id == id))
    }
}

#[async_trait]
impl CrudOperations<TodoistProject> for TodoistProjectDataSource {
    async fn set_field(&self, _id: &str, field: &str, value: Value) -> Result<OperationResult> {
        match field {
            "name" => {
                if let Value::String(_name) = value {
                    // TODO: Implement project_update command in client
                    // For now, just sync to refresh cache
                    use holon::core::datasource::DataSource;
                    let _ = <Self as DataSource<TodoistProject>>::get_all(self).await?;
                }
            }
            _ => {
                return Err(format!("Field {} not supported for projects", field).into());
            }
        }
        // Project operations cannot be undone (they're external API calls)
        Ok(OperationResult::irreversible(vec![]))
    }

    async fn create(&self, fields: HashMap<String, Value>) -> Result<(String, OperationResult)> {
        let name = fields
            .get("name")
            .and_then(|v| v.as_string().map(|s| s.to_string()))
            .ok_or_else(|| "Missing name field".to_string())?;

        // Create project via Sync API
        let project_id = self.provider.client.create_project(&name).await?;

        // Sync to get the full project details
        let sync_resp = self.provider.client.sync_projects(None).await?;
        let projects_array = sync_resp
            .get("projects")
            .and_then(|p| p.as_array())
            .ok_or_else(|| "No projects array in response".to_string())?;

        // Find the created project (no need to cache it)
        if let Some(project_json) = projects_array
            .iter()
            .find(|p| p.get("id").and_then(|id| id.as_str()) == Some(&project_id))
        {
            // Verify project was created successfully
            if serde_json::from_value::<TodoistProjectApiResponse>(project_json.clone()).is_err() {
                return Err("Failed to parse created project".to_string().into());
            }
        }

        Ok((project_id, OperationResult::irreversible(vec![])))
    }

    async fn delete(&self, id: &str) -> Result<OperationResult> {
        self.provider.client.delete_project(id).await?;
        Ok(OperationResult::irreversible(vec![]))
    }
}

#[async_trait]
impl OperationProvider for TodoistProjectDataSource {
    fn operations(&self) -> Vec<OperationDescriptor> {
        vec![
            OperationDescriptor {
                entity_name: "todoist_project".into(),
                entity_short_name: "project".to_string(),
                id_column: "id".to_string(),
                name: "move_block".to_string(),
                display_name: "Move Project".to_string(),
                description: "Move a project under another project".to_string(),
                required_params: vec![
                    OperationParam {
                        name: "id".to_string(),
                        type_hint: TypeHint::String,
                        description: "The project ID to move".to_string(),
                    },
                    OperationParam {
                        name: "parent_id".to_string(),
                        type_hint: TypeHint::EntityId {
                            entity_name: "todoist_project".into(),
                        },
                        description: "The parent project ID (or null for root)".to_string(),
                    },
                ],
                affected_fields: vec!["parent_id".to_string()],
                param_mappings: vec![
                    // From tree drop - project_id triggers this operation
                    ParamMapping {
                        from: "project_id".to_string(),
                        provides: vec!["parent_id".to_string()],
                        defaults: Default::default(),
                    },
                ],
                ..Default::default()
            },
            OperationDescriptor {
                entity_name: "todoist_project".into(),
                entity_short_name: "project".to_string(),
                id_column: "id".to_string(),
                name: "archive".to_string(),
                display_name: "Archive Project".to_string(),
                description: "Archive a project and its descendants".to_string(),
                required_params: vec![OperationParam {
                    name: "id".to_string(),
                    type_hint: TypeHint::String,
                    description: "The project ID to archive".to_string(),
                }],
                affected_fields: vec!["is_archived".to_string()],
                param_mappings: vec![],
                ..Default::default()
            },
            OperationDescriptor {
                entity_name: "todoist_project".into(),
                entity_short_name: "project".to_string(),
                id_column: "id".to_string(),
                name: "unarchive".to_string(),
                display_name: "Unarchive Project".to_string(),
                description: "Unarchive a project".to_string(),
                required_params: vec![OperationParam {
                    name: "id".to_string(),
                    type_hint: TypeHint::String,
                    description: "The project ID to unarchive".to_string(),
                }],
                affected_fields: vec!["is_archived".to_string()],
                param_mappings: vec![],
                ..Default::default()
            },
        ]
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        if entity_name != "todoist_project" {
            return Err(format!(
                "Expected entity_name 'todoist_project', got '{}'",
                entity_name
            )
            .into());
        }

        // Project operations cannot be undone (they're external API calls)
        match op_name {
            "move_block" => {
                self.move_project(&params).await?;
                Ok(OperationResult::irreversible(vec![]))
            }
            "archive" => {
                self.archive_project(&params).await?;
                Ok(OperationResult::irreversible(vec![]))
            }
            "unarchive" => {
                self.unarchive_project(&params).await?;
                Ok(OperationResult::irreversible(vec![]))
            }
            _ => Err(format!("Unknown operation '{}' for todoist_project", op_name).into()),
        }
    }
}

impl TodoistProjectDataSource {
    /// Move a project under another project (or to root)
    async fn move_project(&self, params: &StorageEntity) -> Result<()> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .ok_or_else(|| "move_block requires 'id' parameter")?;

        // parent_id can be null (move to root) or a project ID
        let new_parent_id = params.get("parent_id").and_then(|v| v.as_string());

        debug!(
            "[TodoistProjectDataSource] Moving project {} to parent {:?}",
            id, new_parent_id
        );

        self.provider.client.move_project(id, new_parent_id).await?;

        // Note: sync is now handled automatically by OperationWrapper

        Ok(())
    }

    /// Archive a project and its descendants
    async fn archive_project(&self, params: &StorageEntity) -> Result<()> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .ok_or_else(|| "archive requires 'id' parameter")?;

        debug!("[TodoistProjectDataSource] Archiving project {}", id);

        self.provider.client.archive_project(id).await?;

        // Note: sync is now handled automatically by OperationWrapper

        Ok(())
    }

    /// Unarchive a project
    async fn unarchive_project(&self, params: &StorageEntity) -> Result<()> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .ok_or_else(|| "unarchive requires 'id' parameter")?;

        debug!("[TodoistProjectDataSource] Unarchiving project {}", id);

        self.provider.client.unarchive_project(id).await?;

        // Note: sync is now handled automatically by OperationWrapper

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simple empty DataSource for testing operations structure
    struct EmptyDataSource;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl holon::core::datasource::DataSource<TodoistTask> for EmptyDataSource {
        async fn get_all(&self) -> holon::core::datasource::Result<Vec<TodoistTask>> {
            Ok(Vec::new())
        }

        async fn get_by_id(
            &self,
            _id: &str,
        ) -> holon::core::datasource::Result<Option<TodoistTask>> {
            Ok(None)
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl holon::core::datasource::TaskOperations<TodoistTask> for EmptyDataSource {
        fn completion_states_with_progress(
            &self,
        ) -> Vec<holon::core::datasource::CompletionStateInfo> {
            vec![
                holon::core::datasource::CompletionStateInfo {
                    state: "active".into(),
                    progress: 0.0,
                    is_done: false,
                    is_active: true,
                },
                holon::core::datasource::CompletionStateInfo {
                    state: "completed".into(),
                    progress: 100.0,
                    is_done: true,
                    is_active: false,
                },
            ]
        }

        async fn set_title(
            &self,
            _id: &str,
            _title: &str,
        ) -> holon::core::datasource::Result<holon::core::datasource::OperationResult> {
            unreachable!("Not used in tests")
        }

        async fn set_state(
            &self,
            _id: &str,
            _task_state: String,
        ) -> holon::core::datasource::Result<holon::core::datasource::OperationResult> {
            unreachable!("Not used in tests")
        }

        async fn set_priority(
            &self,
            _id: &str,
            _priority: i64,
        ) -> holon::core::datasource::Result<holon::core::datasource::OperationResult> {
            unreachable!("Not used in tests")
        }

        async fn set_due_date(
            &self,
            _id: &str,
            _due_date: Option<chrono::DateTime<chrono::Utc>>,
        ) -> holon::core::datasource::Result<holon::core::datasource::OperationResult> {
            unreachable!("Not used in tests")
        }
    }

    #[test]
    fn test_operations_with_param_mappings_includes_move_block() {
        let empty_ds = EmptyDataSource;
        let ops = operations_with_resolver(&empty_ds);

        // Find move_block operation
        let move_block = ops.iter().find(|op| op.name == "move_block");
        assert!(move_block.is_some(), "move_block operation should exist");

        let move_block = move_block.unwrap();

        // Check it has exactly 2 required params: id and parent_id
        let param_names: Vec<&str> = move_block
            .required_params
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        println!("move_block required_params: {:?}", param_names);

        assert!(
            param_names.contains(&"id"),
            "move_block should have 'id' param"
        );
        assert!(
            param_names.contains(&"parent_id"),
            "move_block should have 'parent_id' param"
        );
        assert!(
            !param_names.contains(&"after_block_id"),
            "move_block should NOT have 'after_block_id' as required param, but got: {:?}",
            param_names
        );

        // Check param_mappings
        println!("move_block param_mappings: {:?}", move_block.param_mappings);
        assert_eq!(
            move_block.param_mappings.len(),
            2,
            "move_block should have 2 param_mappings (tree_position and selected_id)"
        );

        // Find tree_position mapping
        let tree_position_mapping = move_block
            .param_mappings
            .iter()
            .find(|m| m.from == "tree_position")
            .expect("should have tree_position mapping");
        assert!(
            tree_position_mapping
                .provides
                .contains(&"parent_id".to_string())
        );
    }

    #[test]
    fn test_move_block_should_not_have_after_block_id_as_required() {
        let empty_ds = EmptyDataSource;
        let ops = operations_with_resolver(&empty_ds);
        let move_block = ops.iter().find(|op| op.name == "move_block").unwrap();

        let param_names: Vec<&str> = move_block
            .required_params
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        println!("move_block required_params: {:?}", param_names);
        // after_block_id should NOT be required (it's optional in the trait)
        assert!(
            !param_names.contains(&"after_block_id"),
            "move_block should NOT have 'after_block_id' as required param, but got: {:?}",
            param_names
        );
    }

    #[test]
    fn test_mcp_handled_ops_excluded_from_operations() {
        let empty_ds = EmptyDataSource;
        let ops = operations_with_resolver(&empty_ds);
        let op_names: Vec<&str> = ops.iter().map(|op| op.name.as_str()).collect();

        for excluded in MCP_HANDLED_OPS {
            assert!(
                !op_names.contains(excluded),
                "operation '{}' should be excluded (handled by MCP), but found in: {:?}",
                excluded,
                op_names
            );
        }

        // set_state should still be present
        assert!(
            op_names.contains(&"set_state"),
            "set_state should still be present, got: {:?}",
            op_names
        );
    }

    #[test]
    fn test_all_operations_includes_crud() {
        let empty_ds = EmptyDataSource;
        let ops = all_operations_with_resolver(&empty_ds);
        let op_names: Vec<&str> = ops.iter().map(|op| op.name.as_str()).collect();

        assert!(
            op_names.contains(&"set_field"),
            "all_operations should include set_field"
        );
        assert!(
            op_names.contains(&"create"),
            "all_operations should include create"
        );
        assert!(
            op_names.contains(&"delete"),
            "all_operations should include delete"
        );
    }
}
