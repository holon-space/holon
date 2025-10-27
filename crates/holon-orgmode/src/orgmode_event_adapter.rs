//! Adapter that subscribes to OrgMode changes and publishes to EventBus
//!
//! This adapter bridges the gap between OrgModeSyncProvider's broadcast channels
//! and the EventBus. It subscribes to OrgMode changes (directories, files, blocks)
//! and converts them to Events for publishing to the EventBus.
//!
//! Events are batched before publishing to reduce IVM (Incremental View Maintenance)
//! overhead. This helps avoid concurrent IVM operations that can cause btree panics
//! in Turso.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::time::interval;
use tracing;

use holon::storage::types::Result;
use holon::sync::event_bus::{
    change_to_event, AggregateType, Event, EventBus, EventOrigin, PublishErrorTracker,
};
use holon_filesystem::directory::{ChangesWithMetadata, Directory};
use holon_filesystem::File;

/// Batch size for event publishing. Events are published when this many are accumulated.
const BATCH_SIZE: usize = 50;

/// Maximum delay before flushing a partial batch (in milliseconds).
const BATCH_FLUSH_DELAY_MS: u64 = 100;

/// Adapter that subscribes to OrgMode changes and publishes to EventBus
pub struct OrgModeEventAdapter {
    event_bus: Arc<dyn EventBus>,
    error_tracker: PublishErrorTracker,
}

impl OrgModeEventAdapter {
    /// Create a new OrgModeEventAdapter with a default error tracker
    pub fn new(event_bus: Arc<dyn EventBus>) -> Self {
        Self {
            event_bus,
            error_tracker: PublishErrorTracker::new(),
        }
    }

    /// Create a new OrgModeEventAdapter with a shared error tracker
    ///
    /// Use this when you need to monitor publish errors from tests or DI.
    pub fn with_error_tracker(
        event_bus: Arc<dyn EventBus>,
        error_tracker: PublishErrorTracker,
    ) -> Self {
        Self {
            event_bus,
            error_tracker,
        }
    }

    /// Get the error tracker for monitoring publish errors
    pub fn error_tracker(&self) -> &PublishErrorTracker {
        &self.error_tracker
    }

    /// Start subscribing to OrgMode changes and publishing to EventBus
    ///
    /// This spawns background tasks that listen to the OrgMode broadcast channels
    /// (directories, files, blocks) and publish events to the EventBus.
    ///
    /// Events are batched before publishing to reduce IVM overhead and avoid
    /// concurrent IVM panics in Turso.
    pub fn start(
        &self,
        mut dir_rx: broadcast::Receiver<ChangesWithMetadata<Directory>>,
        mut file_rx: broadcast::Receiver<ChangesWithMetadata<File>>,
    ) -> Result<()> {
        let event_bus = Arc::clone(&self.event_bus);
        let error_tracker = self.error_tracker.clone();

        // Spawn task for directory changes (batched)
        {
            let event_bus_clone = event_bus.clone();
            let tracker = error_tracker.clone();
            tokio::spawn(async move {
                tracing::info!("[OrgModeEventAdapter] Started listening to directory changes");
                let mut event_buffer: Vec<Event> = Vec::with_capacity(BATCH_SIZE);
                let mut flush_timer = interval(Duration::from_millis(BATCH_FLUSH_DELAY_MS));

                loop {
                    tokio::select! {
                        result = dir_rx.recv() => {
                            match result {
                                Ok(batch) => {
                                    for change in batch.inner {
                                        match change_to_event(&change, AggregateType::Directory, EventOrigin::Org, |d: &Directory| d.id.clone()) {
                                            Ok(event) => event_buffer.push(event),
                                            Err(e) => {
                                                tracker.record_error();
                                                tracing::error!(
                                                    "[OrgModeEventAdapter] Failed to convert directory change: {}",
                                                    e
                                                );
                                            }
                                        }
                                    }
                                    if event_buffer.len() >= BATCH_SIZE {
                                        Self::flush_batch(&event_bus_clone, &tracker, &mut event_buffer).await;
                                    }
                                }
                                Err(broadcast::error::RecvError::Lagged(n)) => {
                                    tracing::warn!(
                                        "[OrgModeEventAdapter] Directory stream lagged by {} messages",
                                        n
                                    );
                                }
                                Err(broadcast::error::RecvError::Closed) => {
                                    // Flush remaining events before exit
                                    Self::flush_batch(&event_bus_clone, &tracker, &mut event_buffer).await;
                                    tracing::info!("[OrgModeEventAdapter] Directory stream closed");
                                    break;
                                }
                            }
                        }
                        _ = flush_timer.tick() => {
                            if !event_buffer.is_empty() {
                                Self::flush_batch(&event_bus_clone, &tracker, &mut event_buffer).await;
                            }
                        }
                    }
                }
            });
        }

        // Spawn task for file changes (batched)
        {
            let event_bus_clone = event_bus.clone();
            let tracker = error_tracker.clone();
            tokio::spawn(async move {
                tracing::info!("[OrgModeEventAdapter] Started listening to file changes");
                let mut event_buffer: Vec<Event> = Vec::with_capacity(BATCH_SIZE);
                let mut flush_timer = interval(Duration::from_millis(BATCH_FLUSH_DELAY_MS));

                loop {
                    tokio::select! {
                        result = file_rx.recv() => {
                            match result {
                                Ok(batch) => {
                                    for change in batch.inner {
                                        match change_to_event(&change, AggregateType::File, EventOrigin::Org, |f: &File| f.id.clone()) {
                                            Ok(event) => event_buffer.push(event),
                                            Err(e) => {
                                                tracker.record_error();
                                                tracing::error!(
                                                    "[OrgModeEventAdapter] Failed to convert file change: {}",
                                                    e
                                                );
                                            }
                                        }
                                    }
                                    if event_buffer.len() >= BATCH_SIZE {
                                        Self::flush_batch(&event_bus_clone, &tracker, &mut event_buffer).await;
                                    }
                                }
                                Err(broadcast::error::RecvError::Lagged(n)) => {
                                    tracing::warn!(
                                        "[OrgModeEventAdapter] File stream lagged by {} messages",
                                        n
                                    );
                                }
                                Err(broadcast::error::RecvError::Closed) => {
                                    Self::flush_batch(&event_bus_clone, &tracker, &mut event_buffer).await;
                                    tracing::info!("[OrgModeEventAdapter] File stream closed");
                                    break;
                                }
                            }
                        }
                        _ = flush_timer.tick() => {
                            if !event_buffer.is_empty() {
                                Self::flush_batch(&event_bus_clone, &tracker, &mut event_buffer).await;
                            }
                        }
                    }
                }
            });
        }

        Ok(())
    }

    /// Flush a batch of events to the EventBus
    async fn flush_batch(
        event_bus: &Arc<dyn EventBus>,
        tracker: &PublishErrorTracker,
        buffer: &mut Vec<Event>,
    ) {
        if buffer.is_empty() {
            return;
        }

        let events = std::mem::take(buffer);
        let count = events.len();

        match event_bus.publish_batch(events).await {
            Ok(_) => {
                for _ in 0..count {
                    tracker.record_success();
                }
                tracing::debug!("[OrgModeEventAdapter] Published batch of {} events", count);
            }
            Err(e) => {
                for _ in 0..count {
                    tracker.record_error();
                }
                tracing::error!(
                    "[OrgModeEventAdapter] Failed to publish batch of {} events: {}",
                    count,
                    e
                );
            }
        }
    }
}
