//! QueryableCache Event Subscriber
//!
//! Subscribes to events from the EventBus and ingests them into QueryableCache.
//! Converts Events back to Changes for cache ingestion.

use std::sync::Arc;
use std::time::Duration;
use tokio::time::interval;
use tokio_stream::StreamExt;
use tracing;

use crate::core::queryable_cache::QueryableCache;
use crate::storage::types::{Result, StorageError};
use crate::sync::event_bus::{AggregateType, Event, EventBus, EventFilter, EventKind, EventStatus};
use crate::sync::event_subscriber::EventSubscriber;
use holon_api::block::Block;
use holon_api::streaming::{Change, ChangeOrigin};

/// Maximum delay before flushing a partial batch (in milliseconds).
const BATCH_FLUSH_DELAY_MS: u64 = 50;

/// QueryableCache Event Subscriber
///
/// Subscribes to events from the EventBus and ingests them into QueryableCache.
/// Only processes confirmed events (skips speculative events).
pub struct CacheEventSubscriber {
    block_cache: Arc<QueryableCache<Block>>,
    event_bus: Option<Arc<dyn EventBus>>,
    origin: String,
}

impl CacheEventSubscriber {
    /// Create a new CacheEventSubscriber
    pub fn new(cache: Arc<QueryableCache<Block>>) -> Self {
        Self {
            block_cache: cache,
            event_bus: None,
            origin: "cache".to_string(),
        }
    }

    /// Create a new CacheEventSubscriber with EventBus reference (for mark_processed)
    pub fn with_event_bus(cache: Arc<QueryableCache<Block>>, event_bus: Arc<dyn EventBus>) -> Self {
        Self {
            block_cache: cache,
            event_bus: Some(event_bus),
            origin: "cache".to_string(),
        }
    }

    /// Start subscribing to block events and ingesting to cache.
    ///
    /// For directory/file event subscriptions, use `subscribe_entity()` directly
    /// from the calling code (e.g., frontend wiring).
    pub async fn start(&self, event_bus: Arc<dyn EventBus>) -> Result<()> {
        self.start_block_subscription(event_bus).await
    }

    async fn start_block_subscription(&self, event_bus: Arc<dyn EventBus>) -> Result<()> {
        let cache = Arc::clone(&self.block_cache);

        let filter = EventFilter::new()
            .with_status(EventStatus::Confirmed)
            .with_aggregate_type(AggregateType::Block);

        let mut event_stream = event_bus.subscribe(filter).await?;

        let event_bus_clone = event_bus.clone();
        tokio::spawn(async move {
            tracing::info!("[CacheEventSubscriber] Started listening to block events");

            let mut change_buffer: Vec<Change<Block>> = Vec::new();
            let mut event_ids: Vec<String> = Vec::new();
            let mut flush_timer = interval(Duration::from_millis(BATCH_FLUSH_DELAY_MS));

            loop {
                tokio::select! {
                    maybe_event = event_stream.next() => {
                        match maybe_event {
                            Some(event) => {
                                match Self::event_to_block_change(&event) {
                                    Ok(change) => {
                                        event_ids.push(event.id.clone());
                                        change_buffer.push(change);
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            "[CacheEventSubscriber] Failed to convert block event: {}",
                                            e
                                        );
                                    }
                                }
                            }
                            None => {
                                // Stream closed — flush remaining and exit
                                Self::flush_block_batch(&cache, &event_bus_clone, &mut change_buffer, &mut event_ids).await;
                                tracing::info!("[CacheEventSubscriber] Block event stream closed");
                                break;
                            }
                        }
                    }
                    _ = flush_timer.tick() => {
                        if !change_buffer.is_empty() {
                            Self::flush_block_batch(&cache, &event_bus_clone, &mut change_buffer, &mut event_ids).await;
                        }
                    }
                }
            }
        });

        Ok(())
    }

    /// Flush buffered block changes as a single batch.
    async fn flush_block_batch(
        cache: &Arc<QueryableCache<Block>>,
        event_bus: &Arc<dyn EventBus>,
        changes: &mut Vec<Change<Block>>,
        event_ids: &mut Vec<String>,
    ) {
        let batch = std::mem::take(changes);
        let ids = std::mem::take(event_ids);
        let count = batch.len();

        if count == 0 {
            return;
        }

        let cache_clone = Arc::clone(cache);
        let event_bus_clone = Arc::clone(event_bus);

        // Spawn to avoid deadlock (CDC callback vs TursoEventBus::publish lock)
        tokio::spawn(async move {
            if let Err(e) = cache_clone.apply_batch(&batch, None).await {
                tracing::error!(
                    "[CacheEventSubscriber] Failed to apply block batch of {}: {}",
                    count,
                    e
                );
            } else {
                for id in &ids {
                    if let Err(e) = event_bus_clone.mark_processed(id, "cache").await {
                        tracing::warn!(
                            "[CacheEventSubscriber] Failed to mark event as processed: {}",
                            e
                        );
                    }
                }
            }
        });
    }

    /// Subscribe a QueryableCache to entity events on the EventBus.
    ///
    /// This is a generic helper that works for any entity type (Directory, File, etc.).
    /// Call this from frontend wiring code where both the cache type and EventBus are available.
    pub async fn subscribe_entity<T>(
        aggregate_type: AggregateType,
        cache: Arc<QueryableCache<T>>,
        event_bus: Arc<dyn EventBus>,
    ) -> Result<()>
    where
        T: holon_api::HasSchema
            + serde::Serialize
            + serde::de::DeserializeOwned
            + Clone
            + Send
            + Sync
            + 'static,
    {
        let entity_name = aggregate_type.to_string();
        let filter = EventFilter::new()
            .with_status(EventStatus::Confirmed)
            .with_aggregate_type(aggregate_type);

        let mut event_stream = event_bus.subscribe(filter).await?;
        let event_bus_clone = event_bus.clone();
        tokio::spawn(async move {
            tracing::info!(
                "[CacheEventSubscriber] Started listening to {} events",
                entity_name
            );

            let mut change_buffer: Vec<Change<T>> = Vec::new();
            let mut event_ids: Vec<String> = Vec::new();
            let mut flush_timer = interval(Duration::from_millis(BATCH_FLUSH_DELAY_MS));

            loop {
                tokio::select! {
                    maybe_event = event_stream.next() => {
                        match maybe_event {
                            Some(event) => {
                                match Self::event_to_entity_change::<T>(&event) {
                                    Ok(change) => {
                                        event_ids.push(event.id.clone());
                                        change_buffer.push(change);
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            "[CacheEventSubscriber] Failed to convert {} event: {}",
                                            entity_name,
                                            e
                                        );
                                    }
                                }
                            }
                            None => {
                                Self::flush_entity_batch(&entity_name, &cache, &event_bus_clone, &mut change_buffer, &mut event_ids).await;
                                tracing::info!("[CacheEventSubscriber] {} event stream closed", entity_name);
                                break;
                            }
                        }
                    }
                    _ = flush_timer.tick() => {
                        if !change_buffer.is_empty() {
                            Self::flush_entity_batch(&entity_name, &cache, &event_bus_clone, &mut change_buffer, &mut event_ids).await;
                        }
                    }
                }
            }
        });

        Ok(())
    }

    /// Flush buffered entity changes as a single batch.
    async fn flush_entity_batch<T>(
        entity_name: &str,
        cache: &Arc<QueryableCache<T>>,
        event_bus: &Arc<dyn EventBus>,
        changes: &mut Vec<Change<T>>,
        event_ids: &mut Vec<String>,
    ) where
        T: holon_api::HasSchema
            + serde::Serialize
            + serde::de::DeserializeOwned
            + Clone
            + Send
            + Sync
            + 'static,
    {
        let batch = std::mem::take(changes);
        let ids = std::mem::take(event_ids);
        let count = batch.len();

        if count == 0 {
            return;
        }

        if let Err(e) = cache.apply_batch(&batch, None).await {
            tracing::error!(
                "[CacheEventSubscriber] Failed to apply {} batch of {}: {}",
                entity_name,
                count,
                e
            );
        } else {
            for id in &ids {
                if let Err(e) = event_bus.mark_processed(id, "cache").await {
                    tracing::warn!(
                        "[CacheEventSubscriber] Failed to mark event as processed: {}",
                        e
                    );
                }
            }
        }
    }

    /// Convert an Event to a Change for a generic entity type
    fn event_to_entity_change<T: serde::de::DeserializeOwned>(event: &Event) -> Result<Change<T>> {
        let change_origin = Self::extract_change_origin(event);

        match event.event_kind {
            EventKind::Created => {
                let data_value = event.payload.get("data").ok_or_else(|| {
                    StorageError::DatabaseError("Missing 'data' in event payload".to_string())
                })?;
                let data: T = serde_json::from_value(data_value.clone()).map_err(|e| {
                    StorageError::SerializationError(format!("Failed to deserialize: {}", e))
                })?;
                Ok(Change::Created {
                    data,
                    origin: change_origin,
                })
            }
            EventKind::Updated => {
                let data_value = event.payload.get("data").ok_or_else(|| {
                    StorageError::DatabaseError("Missing 'data' in event payload".to_string())
                })?;
                let data: T = serde_json::from_value(data_value.clone()).map_err(|e| {
                    StorageError::SerializationError(format!("Failed to deserialize: {}", e))
                })?;
                Ok(Change::Updated {
                    id: event.aggregate_id.clone(),
                    data,
                    origin: change_origin,
                })
            }
            EventKind::Deleted => Ok(Change::Deleted {
                id: event.aggregate_id.clone(),
                origin: change_origin,
            }),
            EventKind::FieldsChanged => {
                let fields_value = event.payload.get("fields").ok_or_else(|| {
                    StorageError::DatabaseError("Missing 'fields' in event payload".to_string())
                })?;
                let fields: Vec<(String, holon_api::Value, holon_api::Value)> =
                    serde_json::from_value(fields_value.clone()).map_err(|e| {
                        StorageError::SerializationError(format!(
                            "Failed to deserialize fields: {}",
                            e
                        ))
                    })?;
                Ok(Change::FieldsChanged {
                    entity_id: event.aggregate_id.clone(),
                    fields,
                    origin: change_origin,
                })
            }
        }
    }

    fn extract_change_origin(event: &Event) -> ChangeOrigin {
        match event.origin {
            crate::sync::event_bus::EventOrigin::Ui => ChangeOrigin::Local {
                operation_id: None,
                trace_id: event.trace_id.clone(),
            },
            _ => ChangeOrigin::Remote {
                operation_id: None,
                trace_id: event.trace_id.clone(),
            },
        }
    }

    /// Convert an Event back to a Change<Block>
    fn event_to_block_change(event: &Event) -> Result<Change<Block>> {
        let change_origin = match event.origin {
            crate::sync::event_bus::EventOrigin::Ui => ChangeOrigin::Local {
                operation_id: None,
                trace_id: event.trace_id.clone(),
            },
            _ => ChangeOrigin::Remote {
                operation_id: None,
                trace_id: event.trace_id.clone(),
            },
        };

        match event.event_kind {
            EventKind::Created => {
                let data_value = event.payload.get("data").ok_or_else(|| {
                    StorageError::DatabaseError("Missing 'data' in event payload".to_string())
                })?;
                let block: Block = serde_json::from_value(data_value.clone()).map_err(|e| {
                    StorageError::SerializationError(format!("Failed to deserialize Block: {}", e))
                })?;
                Ok(Change::Created {
                    data: block,
                    origin: change_origin,
                })
            }
            EventKind::Updated => {
                let data_value = event.payload.get("data").ok_or_else(|| {
                    StorageError::DatabaseError("Missing 'data' in event payload".to_string())
                })?;
                let block: Block = serde_json::from_value(data_value.clone()).map_err(|e| {
                    StorageError::SerializationError(format!("Failed to deserialize Block: {}", e))
                })?;
                Ok(Change::Updated {
                    id: event.aggregate_id.clone(),
                    data: block,
                    origin: change_origin,
                })
            }
            EventKind::Deleted => Ok(Change::Deleted {
                id: event.aggregate_id.clone(),
                origin: change_origin,
            }),
            EventKind::FieldsChanged => {
                let fields_value = event.payload.get("fields").ok_or_else(|| {
                    StorageError::DatabaseError("Missing 'fields' in event payload".to_string())
                })?;
                let fields: Vec<(String, holon_api::Value, holon_api::Value)> =
                    serde_json::from_value(fields_value.clone()).map_err(|e| {
                        StorageError::SerializationError(format!(
                            "Failed to deserialize fields: {}",
                            e
                        ))
                    })?;
                Ok(Change::FieldsChanged {
                    entity_id: event.aggregate_id.clone(),
                    fields,
                    origin: change_origin,
                })
            }
        }
    }
}

#[async_trait::async_trait]
impl EventSubscriber for CacheEventSubscriber {
    fn origin(&self) -> &str {
        &self.origin
    }

    async fn process_event(&self, event: &Event) -> Result<()> {
        let change = Self::event_to_block_change(event)?;
        self.block_cache
            .apply_batch(&[change], None)
            .await
            .map_err(|e| {
                StorageError::DatabaseError(format!("Failed to apply batch to cache: {}", e))
            })?;

        // Mark event as processed if EventBus reference is available
        if let Some(ref event_bus) = self.event_bus {
            if let Err(e) = event_bus.mark_processed(&event.id, "cache").await {
                tracing::warn!(
                    "[CacheEventSubscriber] Failed to mark event as processed: {}",
                    e
                );
            }
        }

        Ok(())
    }
}
