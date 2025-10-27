//! Adapter that subscribes to Todoist changes and publishes to EventBus
//!
//! This adapter bridges the gap between TodoistSyncProvider's broadcast channels
//! and the EventBus. It subscribes to Todoist changes and converts them to Events
//! for publishing to the EventBus.
//!
//! Per Q4 decision: Cache writes happen directly via QueryableCache subscription
//! (for speed with sync tokens), while this adapter publishes events to EventBus
//! (for audit/replay).

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing;

use holon::storage::types::{Result, StorageError};
use holon::sync::event_bus::{AggregateType, Event, EventBus, EventKind, EventOrigin};
use holon_api::streaming::{Change, ChangeOrigin};

use crate::models::{TodoistProject, TodoistTask};
use crate::todoist_sync_provider::ChangesWithMetadata;

/// Adapter that subscribes to Todoist changes and publishes to EventBus
///
/// Per Q4 decision: Cache writes happen directly via QueryableCache subscription
/// (handled separately in DI wiring). This adapter only publishes events to EventBus
/// for audit/replay.
pub struct TodoistEventAdapter {
    event_bus: Arc<dyn EventBus>,
}

impl TodoistEventAdapter {
    /// Create a new TodoistEventAdapter
    pub fn new(event_bus: Arc<dyn EventBus>) -> Self {
        Self { event_bus }
    }

    /// Start subscribing to Todoist changes and publishing to EventBus
    ///
    /// This spawns background tasks that listen to the Todoist broadcast channels
    /// and publish events to the EventBus.
    ///
    /// Note: Cache writes are handled separately via QueryableCache subscription
    /// (wired in DI module) to ensure sync tokens are handled atomically.
    pub fn start(
        &self,
        mut task_rx: broadcast::Receiver<ChangesWithMetadata<TodoistTask>>,
        mut project_rx: broadcast::Receiver<ChangesWithMetadata<TodoistProject>>,
    ) -> Result<()> {
        let event_bus_task = Arc::clone(&self.event_bus);
        let event_bus_project = Arc::clone(&self.event_bus);

        // Spawn task for task changes
        tokio::spawn(async move {
            tracing::info!("[TodoistEventAdapter] Started listening to Todoist task changes");

            loop {
                match task_rx.recv().await {
                    Ok(batch) => {
                        // Extract changes
                        let changes = batch.inner;

                        // Publish events to EventBus
                        for change in changes {
                            if let Err(e) =
                                Self::publish_task_change(&event_bus_task, &change).await
                            {
                                tracing::error!(
                                    "[TodoistEventAdapter] Failed to publish task change: {}",
                                    e
                                );
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(
                            "[TodoistEventAdapter] Task stream lagged by {} messages",
                            n
                        );
                        // Continue processing - don't break on lag
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::info!("[TodoistEventAdapter] Task stream closed");
                        break;
                    }
                }
            }
        });

        // Spawn task for project changes
        tokio::spawn(async move {
            tracing::info!("[TodoistEventAdapter] Started listening to Todoist project changes");

            loop {
                match project_rx.recv().await {
                    Ok(batch) => {
                        // Extract changes
                        let changes = batch.inner;

                        // Publish events to EventBus
                        for change in changes {
                            if let Err(e) =
                                Self::publish_project_change(&event_bus_project, &change).await
                            {
                                tracing::error!(
                                    "[TodoistEventAdapter] Failed to publish project change: {}",
                                    e
                                );
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(
                            "[TodoistEventAdapter] Project stream lagged by {} messages",
                            n
                        );
                        // Continue processing - don't break on lag
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::info!("[TodoistEventAdapter] Project stream closed");
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    /// Convert a Change<T> to an Event and publish it
    async fn publish_change<T: serde::Serialize>(
        event_bus: &Arc<dyn EventBus>,
        change: &Change<T>,
        aggregate_type: AggregateType,
        extract_id: impl Fn(&T) -> String,
    ) -> Result<()> {
        let (event_kind, aggregate_id, payload_map, trace_id) = match change {
            Change::Created { data, origin } => {
                let payload = serde_json::to_value(data).map_err(|e| {
                    StorageError::SerializationError(format!("Failed to serialize: {}", e))
                })?;
                let mut payload_map = HashMap::new();
                payload_map.insert("data".to_string(), payload);
                payload_map.insert(
                    "change_type".to_string(),
                    serde_json::Value::String("created".to_string()),
                );
                let trace_id = match origin {
                    ChangeOrigin::Local { trace_id, .. }
                    | ChangeOrigin::Remote { trace_id, .. } => trace_id.clone(),
                };
                (EventKind::Created, extract_id(data), payload_map, trace_id)
            }
            Change::Updated { id, data, origin } => {
                let payload = serde_json::to_value(data).map_err(|e| {
                    StorageError::SerializationError(format!("Failed to serialize: {}", e))
                })?;
                let mut payload_map = HashMap::new();
                payload_map.insert("data".to_string(), payload);
                payload_map.insert(
                    "change_type".to_string(),
                    serde_json::Value::String("updated".to_string()),
                );
                let trace_id = match origin {
                    ChangeOrigin::Local { trace_id, .. }
                    | ChangeOrigin::Remote { trace_id, .. } => trace_id.clone(),
                };
                (EventKind::Updated, id.clone(), payload_map, trace_id)
            }
            Change::Deleted { id, origin } => {
                let mut payload_map = HashMap::new();
                payload_map.insert(
                    "change_type".to_string(),
                    serde_json::Value::String("deleted".to_string()),
                );
                let trace_id = match origin {
                    ChangeOrigin::Local { trace_id, .. }
                    | ChangeOrigin::Remote { trace_id, .. } => trace_id.clone(),
                };
                (EventKind::Deleted, id.clone(), payload_map, trace_id)
            }
            Change::FieldsChanged {
                entity_id,
                fields,
                origin,
            } => {
                let fields_json = serde_json::to_value(fields).map_err(|e| {
                    StorageError::SerializationError(format!("Failed to serialize fields: {}", e))
                })?;
                let mut payload_map = HashMap::new();
                payload_map.insert("fields".to_string(), fields_json);
                payload_map.insert(
                    "change_type".to_string(),
                    serde_json::Value::String("fields_changed".to_string()),
                );
                let trace_id = match origin {
                    ChangeOrigin::Local { trace_id, .. }
                    | ChangeOrigin::Remote { trace_id, .. } => trace_id.clone(),
                };
                (
                    EventKind::FieldsChanged,
                    entity_id.clone(),
                    payload_map,
                    trace_id,
                )
            }
        };

        let mut event = Event::new(
            event_kind,
            aggregate_type,
            aggregate_id,
            EventOrigin::Todoist,
            payload_map,
        );
        event.trace_id = trace_id;

        event_bus.publish(event, None).await?;
        Ok(())
    }

    /// Convert a TodoistTask Change to an Event and publish it
    async fn publish_task_change(
        event_bus: &Arc<dyn EventBus>,
        change: &Change<TodoistTask>,
    ) -> Result<()> {
        Self::publish_change(event_bus, change, AggregateType::Task, |t| t.id.clone()).await
    }

    /// Convert a TodoistProject Change to an Event and publish it
    async fn publish_project_change(
        event_bus: &Arc<dyn EventBus>,
        change: &Change<TodoistProject>,
    ) -> Result<()> {
        Self::publish_change(event_bus, change, AggregateType::Project, |p| p.id.clone()).await
    }
}
