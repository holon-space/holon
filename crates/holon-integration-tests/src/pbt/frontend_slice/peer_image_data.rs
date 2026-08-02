//! The image-bytes half of a synced peer.
//!
//! `FileSyncController` materializes an image block's bytes to
//! `<root>/<block.content>`. Both halves of that write — the PATH (the block's
//! content) and the BYTES — arrive together from whoever authored the block,
//! so a peer that can send a block can send bytes to go with it. Modelling the
//! bytes as always-available is what makes the write reachable in a test; a
//! provider that only ever holds what this process previously ingested from
//! disk could never express "a peer sent us an image we do not have".

use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::Result;
use async_trait::async_trait;
use holon_api::EntityUri;
use holon_filesystem::ImageDataProvider;

#[derive(Default)]
pub struct PeerImageData {
    stored: Mutex<HashMap<String, Vec<u8>>>,
}

#[async_trait]
impl ImageDataProvider for PeerImageData {
    async fn read_image_data(&self, block_id: &EntityUri) -> Result<Option<Vec<u8>>> {
        let key = block_id.to_string();
        let mut stored = self.stored.lock().expect("PeerImageData mutex poisoned");
        // Deterministic per-block bytes, minted on first read: every image block
        // in a run has bytes, as if its author had sent them.
        Ok(Some(
            stored
                .entry(key.clone())
                .or_insert_with(|| format!("PNG:{key}").into_bytes())
                .clone(),
        ))
    }

    async fn write_image_data(&self, block_id: &EntityUri, data: Vec<u8>) -> Result<()> {
        self.stored
            .lock()
            .expect("PeerImageData mutex poisoned")
            .insert(block_id.to_string(), data);
        Ok(())
    }
}
