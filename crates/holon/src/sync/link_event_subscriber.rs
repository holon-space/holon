//! Link Event Subscriber
//!
//! Subscribes to block events from the EventBus, extracts `[[...]]` links
//! from block content, and populates the `block_link` table.
//! Uses deterministic hashing for link target resolution — no DB queries needed.

use std::sync::Arc;

use tokio_stream::StreamExt;

use crate::storage::turso::DbHandle;
use crate::storage::types::Result;
use crate::sync::event_bus::{AggregateType, EventBus, EventFilter, EventKind, EventStatus};
use holon_api::link_parser::extract_links;

fn text(s: &str) -> turso::Value {
    turso::Value::Text(s.to_string())
}

/// Subscribes to block events to maintain the `block_link` table.
///
/// Link target resolution is purely deterministic (blake3 hash of normalized path),
/// so no document lookups or deferred resolution is needed.
pub struct LinkEventSubscriber {
    db_handle: DbHandle,
}

impl LinkEventSubscriber {
    pub fn new(db_handle: DbHandle) -> Self {
        Self { db_handle }
    }

    pub async fn start(&self, event_bus: Arc<dyn EventBus>) -> Result<()> {
        let db = self.db_handle.clone();

        let filter = EventFilter::new()
            .with_status(EventStatus::Confirmed)
            .with_aggregate_type(AggregateType::Block);

        let mut event_stream = event_bus.subscribe(filter).await?;

        tokio::spawn(async move {
            tracing::info!("[LinkEventSubscriber] Started listening to block events");

            while let Some(event) = event_stream.next().await {
                let block_id = &event.aggregate_id;
                let result = match event.event_kind {
                    EventKind::Created | EventKind::Updated | EventKind::FieldsChanged => {
                        let content = event
                            .payload
                            .get("data")
                            .and_then(|d| d.get("content"))
                            .and_then(|c| c.as_str())
                            .unwrap_or("");
                        Self::index_links(&db, block_id, content).await
                    }
                    EventKind::Deleted => Self::delete_links(&db, block_id).await,
                };

                if let Err(e) = result {
                    tracing::error!(
                        "[LinkEventSubscriber] Failed to index links for block {}: {}",
                        block_id,
                        e
                    );
                }
            }

            tracing::info!("[LinkEventSubscriber] Block event stream closed");
        });

        Ok(())
    }

    async fn index_links(db: &DbHandle, block_id: &str, content: &str) -> Result<()> {
        let links = extract_links(content);

        db.execute(
            "DELETE FROM block_link WHERE source_block_id = ?",
            vec![text(block_id)],
        )
        .await?;

        for link in &links {
            let target_id = link.classified.entity_id().map(|uri| text(uri.as_str()));

            // Use the user-provided display text only if it differs from the raw target
            let display_text = if link.text != link.target {
                text(&link.text)
            } else {
                turso::Value::Null
            };

            db.execute(
                "INSERT INTO block_link (source_block_id, target_raw, target_id, display_text, position) VALUES (?, ?, ?, ?, ?)",
                vec![
                    text(block_id),
                    text(&link.target),
                    target_id.unwrap_or(turso::Value::Null),
                    display_text,
                    turso::Value::Integer(link.start as i64),
                ],
            )
            .await?;
        }

        if !links.is_empty() {
            tracing::debug!(
                "[LinkEventSubscriber] Indexed {} links for block {}",
                links.len(),
                block_id
            );
        }

        Ok(())
    }

    async fn delete_links(db: &DbHandle, block_id: &str) -> Result<()> {
        db.execute(
            "DELETE FROM block_link WHERE source_block_id = ?",
            vec![text(block_id)],
        )
        .await?;
        Ok(())
    }
}
