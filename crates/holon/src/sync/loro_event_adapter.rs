//! Adapter that subscribes to Loro changes and publishes to EventBus
//!
//! This adapter bridges the gap between LoroBlockOperations' broadcast channel
//! and the EventBus. It subscribes to Loro changes and converts them to Events
//! for publishing to the EventBus.

use std::sync::Arc;
use tokio::sync::broadcast;
use tracing;

use crate::storage::types::Result;
use crate::sync::event_bus::{AggregateType, EventBus, EventOrigin, change_to_event};
use holon_api::block::Block;
use holon_api::streaming::Change;

/// Adapter that subscribes to Loro changes and publishes to EventBus
pub struct LoroEventAdapter {
    event_bus: Arc<dyn EventBus>,
}

impl LoroEventAdapter {
    /// Create a new LoroEventAdapter
    pub fn new(event_bus: Arc<dyn EventBus>) -> Self {
        Self { event_bus }
    }

    /// Start subscribing to Loro changes and publishing to EventBus
    ///
    /// This spawns a background task that listens to the Loro broadcast channel
    /// and publishes events to the EventBus.
    pub fn start(&self, mut loro_rx: broadcast::Receiver<Vec<Change<Block>>>) -> Result<()> {
        let event_bus = Arc::clone(&self.event_bus);

        tokio::spawn(async move {
            tracing::info!("[LoroEventAdapter] Started listening to Loro changes");

            loop {
                match loro_rx.recv().await {
                    Ok(changes) => {
                        for change in changes {
                            let event = change_to_event(
                                &change,
                                AggregateType::Block,
                                EventOrigin::Loro,
                                |b: &Block| b.id.to_string(),
                            );
                            match event {
                                Ok(event) => {
                                    if let Err(e) = event_bus.publish(event, None).await {
                                        tracing::error!(
                                            "[LoroEventAdapter] Failed to publish change: {}",
                                            e
                                        );
                                    }
                                }
                                Err(e) => {
                                    tracing::error!(
                                        "[LoroEventAdapter] Failed to convert change: {}",
                                        e
                                    );
                                }
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("[LoroEventAdapter] Stream lagged by {} messages", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::info!("[LoroEventAdapter] Loro stream closed");
                        break;
                    }
                }
            }
        });

        Ok(())
    }
}
