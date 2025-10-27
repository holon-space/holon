//! Adapter that subscribes to Todoist changes and publishes to EventBus
//!
//! This adapter bridges the gap between TodoistSyncProvider's broadcast channels
//! and the EventBus. It subscribes to Todoist changes and converts them to Events
//! for publishing to the EventBus.
//!
//! Per Q4 decision: Cache writes happen directly via QueryableCache subscription
//! (for speed with sync tokens), while this adapter publishes events to EventBus
//! (for audit/replay).

use std::sync::Arc;
use tokio::sync::broadcast;
use tracing;

use holon::storage::types::Result;
use holon::sync::event_bus::{AggregateType, EventBus, EventOrigin, change_to_event};

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
                        for change in batch.inner {
                            let event = change_to_event(
                                &change,
                                AggregateType::Task,
                                EventOrigin::Todoist,
                                |t: &TodoistTask| t.id.clone(),
                            );
                            match event {
                                Ok(event) => {
                                    if let Err(e) = event_bus_task.publish(event, None).await {
                                        tracing::error!(
                                            "[TodoistEventAdapter] Failed to publish task change: {}",
                                            e
                                        );
                                    }
                                }
                                Err(e) => {
                                    tracing::error!(
                                        "[TodoistEventAdapter] Failed to convert task change: {}",
                                        e
                                    );
                                }
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(
                            "[TodoistEventAdapter] Task stream lagged by {} messages",
                            n
                        );
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
                        for change in batch.inner {
                            let event = change_to_event(
                                &change,
                                AggregateType::Project,
                                EventOrigin::Todoist,
                                |p: &TodoistProject| p.id.clone(),
                            );
                            match event {
                                Ok(event) => {
                                    if let Err(e) = event_bus_project.publish(event, None).await {
                                        tracing::error!(
                                            "[TodoistEventAdapter] Failed to publish project change: {}",
                                            e
                                        );
                                    }
                                }
                                Err(e) => {
                                    tracing::error!(
                                        "[TodoistEventAdapter] Failed to convert project change: {}",
                                        e
                                    );
                                }
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(
                            "[TodoistEventAdapter] Project stream lagged by {} messages",
                            n
                        );
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
}
