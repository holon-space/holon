//! Turso implementation of the [`BlockLinkIndexer`] seam: persists extracted
//! block links into the `block_link` table. Constructed at wiring time
//! (`sync/event_infra_module.rs`); the sync layer's `LinkEventSubscriber`
//! only sees the trait (Phase 2.5, C6 of the architecture plan).

use holon_api::link_parser::Link;

use crate::storage::turso::DbHandle;
use crate::storage::types::Result;
use crate::sync::link_event_subscriber::BlockLinkIndexer;

fn text(s: &str) -> turso::Value {
    turso::Value::Text(s.to_string())
}

pub struct TursoBlockLinkIndexer {
    db_handle: DbHandle,
}

impl TursoBlockLinkIndexer {
    pub fn new(db_handle: DbHandle) -> Self {
        Self { db_handle }
    }
}

#[async_trait::async_trait]
impl BlockLinkIndexer for TursoBlockLinkIndexer {
    async fn replace_links(&self, block_id: &str, links: &[Link]) -> Result<()> {
        self.db_handle
            .execute(
                "DELETE FROM block_link WHERE source_block_id = ?",
                vec![text(block_id)],
            )
            .await?;

        for link in links {
            let target_id = link.classified.entity_id().map(|uri| text(uri.as_str()));

            // Use the user-provided display text only if it differs from the raw target
            let display_text = if link.text != link.target {
                text(&link.text)
            } else {
                turso::Value::Null
            };

            self.db_handle
                .execute(
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

        Ok(())
    }

    async fn delete_links(&self, block_id: &str) -> Result<()> {
        self.db_handle
            .execute(
                "DELETE FROM block_link WHERE source_block_id = ?",
                vec![text(block_id)],
            )
            .await?;
        Ok(())
    }
}
