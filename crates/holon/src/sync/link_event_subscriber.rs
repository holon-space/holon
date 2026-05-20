//! Link Event Subscriber
//!
//! Maintains the block-link index from the convergent downstream
//! [`LiveData<Block>`] feed (the `block` matview's CDC stream) — NOT the
//! EventBus (Phase 4b: sinks are fed by the ack-free convergent stream). Each
//! block insert/update re-extracts `[[...]]` links from its content; each
//! removal drops them. Link target resolution is deterministic (blake3 hash of
//! the normalized path), so no DB lookups are needed.
//!
//! The subscriber owns link *extraction*; persistence goes through the
//! [`BlockLinkIndexer`] seam so this layer never names a concrete sink
//! (Phase 2.5, C6 of the architecture plan — the Turso impl lives in
//! `storage/turso_block_link_indexer.rs`).

use std::sync::Arc;

use futures_signals::signal_map::{MapDiff, SignalMapExt};

use crate::storage::types::Result;
use crate::sync::live_data::LiveData;
use holon_api::block::Block;
use holon_api::link_parser::{Link, extract_links};

/// Write side of the block-link index. Replaces or drops the persisted links
/// of one source block.
#[async_trait::async_trait]
pub trait BlockLinkIndexer: Send + Sync {
    /// Atomically replace all persisted links of `block_id` with `links`.
    async fn replace_links(&self, block_id: &str, links: &[Link]) -> Result<()>;
    /// Drop all persisted links of `block_id`.
    async fn delete_links(&self, block_id: &str) -> Result<()>;
}

/// Subscribes to block events to maintain the block-link index.
///
/// Link target resolution is purely deterministic (blake3 hash of normalized path),
/// so no document lookups or deferred resolution is needed.
pub struct LinkEventSubscriber {
    indexer: Arc<dyn BlockLinkIndexer>,
}

impl LinkEventSubscriber {
    pub fn new(indexer: Arc<dyn BlockLinkIndexer>) -> Self {
        Self { indexer }
    }

    /// Maintain the link index from the convergent [`LiveData<Block>`] feed.
    /// Spawns a background task that reacts to the feed's `MapDiff` stream:
    /// the initial `Replace` re-indexes every block's links (boot consistency),
    /// then `Insert`/`Update` re-index a block and `Remove` drops its links.
    /// No EventBus, no acks — nothing waits on the (removed) `Consumer::LINKS`
    /// watermark.
    pub fn start_from_live_data(&self, block_live: Arc<LiveData<Block>>) {
        let indexer = self.indexer.clone();
        let signal = block_live.signal_map();

        tokio::spawn(async move {
            tracing::info!("[LinkEventSubscriber] Started listening to LiveData<Block> feed");
            signal
                .for_each(move |diff| {
                    let indexer = indexer.clone();
                    async move {
                        let result = match diff {
                            MapDiff::Replace { entries } => {
                                let mut last = Ok(());
                                for (_key, block) in entries {
                                    let r = Self::index_links(
                                        &*indexer,
                                        block.id.as_str(),
                                        &block.content,
                                    )
                                    .await;
                                    if r.is_err() {
                                        last = r;
                                    }
                                }
                                last
                            }
                            MapDiff::Insert { value, .. } | MapDiff::Update { value, .. } => {
                                Self::index_links(&*indexer, value.id.as_str(), &value.content)
                                    .await
                            }
                            MapDiff::Remove { key } => indexer.delete_links(&key).await,
                            MapDiff::Clear {} => Ok(()),
                        };
                        if let Err(e) = result {
                            tracing::error!("[LinkEventSubscriber] Failed to index links: {}", e);
                        }
                    }
                })
                .await;
            tracing::info!("[LinkEventSubscriber] LiveData<Block> feed stream closed");
        });
    }

    async fn index_links(
        indexer: &dyn BlockLinkIndexer,
        block_id: &str,
        content: &str,
    ) -> Result<()> {
        let links = extract_links(content);
        indexer.replace_links(block_id, &links).await?;

        if !links.is_empty() {
            tracing::debug!(
                "[LinkEventSubscriber] Indexed {} links for block {}",
                links.len(),
                block_id
            );
        }

        Ok(())
    }
}
