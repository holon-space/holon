//! Optional Loro validation layer for PBT.
//!
//! When Loro is enabled, reads all blocks from the LoroTree and compares them
//! against the reference model. With stable IDs, blocks already have UUID-based
//! IDs in their CRDT metadata, so no ID normalization is needed.

use std::collections::HashSet;
use std::fmt::Write;
use std::sync::Arc;

use tokio::sync::RwLock;

use holon::api::{CoreOperations, LoroBackend};
use holon::sync::LoroDocumentStore;
use holon_api::EntityUri;
use holon_api::block::Block;

use crate::assertions::normalize_block;

/// Encapsulates Loro-specific PBT validation.
/// With stable IDs, blocks from the LoroTree already carry UUID-based IDs
/// in their metadata — no external_id mapping is needed.
pub struct LoroSut {
    doc_store: Arc<RwLock<LoroDocumentStore>>,
}

impl LoroSut {
    pub fn new(doc_store: Arc<RwLock<LoroDocumentStore>>) -> Self {
        Self { doc_store }
    }

    /// Read all blocks from the LoroTree.
    /// Blocks already have stable UUID-based IDs from their CRDT metadata.
    pub async fn read_blocks(&self) -> anyhow::Result<Vec<Block>> {
        let store = self.doc_store.read().await;
        let collab_doc = store
            .get_global_doc()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get global doc: {}", e))?;

        let backend = LoroBackend::from_document(collab_doc);
        backend
            .get_all_blocks(holon::api::types::Traversal::ALL)
            .await
            .map_err(|e| anyhow::anyhow!("get_all_blocks failed: {}", e))
    }

    /// Assert that the Loro tree matches the reference model.
    ///
    /// Reads all blocks from Loro, then compares against the reference blocks
    /// using the same normalization as SQL checks.
    /// Retries for up to 5s to allow reverse sync to complete.
    pub async fn assert_matches_reference(
        &self,
        ref_blocks: &[Block],
        seed_block_ids: &std::collections::HashSet<EntityUri>,
    ) {
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);

        loop {
            let loro_blocks = match self.read_blocks().await {
                Ok(blocks) => blocks,
                Err(e) => {
                    panic!("[LoroSut] Failed to read Loro blocks: {}", e);
                }
            };

            let loro_filtered: Vec<_> = loro_blocks
                .iter()
                .filter(|b| !seed_block_ids.contains(&b.id))
                .filter(|b| !b.is_page())
                // Exclude page placeholder roots created by reverse sync.
                .filter(|b| {
                    !(b.parent_id.is_no_parent() && b.content.is_empty() && b.tags.is_empty())
                })
                .cloned()
                .collect();

            // Normalize page parent_ids on both sides. Pages are managed
            // separately (DocumentManager) and their identity mapping is tested
            // by the SQL assertions, not the Loro assertion.
            let ref_filtered: Vec<_> = ref_blocks.iter().filter(|b| !b.is_page()).collect();

            let loro_content_ids: HashSet<&EntityUri> =
                loro_filtered.iter().map(|b| &b.id).collect();
            let ref_content_ids: HashSet<&EntityUri> = ref_filtered.iter().map(|b| &b.id).collect();

            let normalize_doc_parent = |block: &Block, content_ids: &HashSet<&EntityUri>| {
                let mut normalized = normalize_block(block);
                if !normalized.parent_id.is_no_parent()
                    && !normalized.parent_id.is_sentinel()
                    && !content_ids.contains(&block.parent_id)
                {
                    normalized.parent_id = EntityUri::block("__document_root__");
                }
                normalized
            };

            let mut loro_sorted: Vec<_> = loro_filtered
                .iter()
                .map(|b| normalize_doc_parent(b, &loro_content_ids))
                .collect();
            let mut ref_sorted: Vec<_> = ref_filtered
                .iter()
                .map(|b| normalize_doc_parent(b, &ref_content_ids))
                .collect();
            loro_sorted.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
            ref_sorted.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));

            // Reverse sync still running — retry until deadline
            if loro_sorted != ref_sorted && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                continue;
            }

            let diagnostic = build_diagnostic(&loro_sorted, &ref_sorted);

            assert_eq!(
                loro_sorted, ref_sorted,
                "[LoroSut] Block content mismatch\n{}",
                diagnostic,
            );

            return;
        }
    }
}

/// Build a diagnostic string showing exactly what differs between Loro and reference.
fn build_diagnostic(loro: &[Block], reference: &[Block]) -> String {
    let mut out = String::new();

    let loro_ids: Vec<_> = loro.iter().map(|b| b.id.as_str()).collect();
    let ref_ids: Vec<_> = reference.iter().map(|b| b.id.as_str()).collect();

    let _ = writeln!(out, "Loro ({} blocks): {:?}", loro.len(), loro_ids);
    let _ = writeln!(out, "Ref  ({} blocks): {:?}", reference.len(), ref_ids);

    // IDs only in one side
    let only_loro: Vec<_> = loro_ids.iter().filter(|id| !ref_ids.contains(id)).collect();
    let only_ref: Vec<_> = ref_ids.iter().filter(|id| !loro_ids.contains(id)).collect();
    if !only_loro.is_empty() {
        let _ = writeln!(out, "Only in Loro: {:?}", only_loro);
    }
    if !only_ref.is_empty() {
        let _ = writeln!(out, "Only in Ref:  {:?}", only_ref);
    }

    // Per-block diffs for shared IDs
    for ref_block in reference {
        if let Some(loro_block) = loro.iter().find(|b| b.id == ref_block.id)
            && loro_block != ref_block
        {
            let _ = writeln!(out, "DIFF {}:", ref_block.id);
            if loro_block.content != ref_block.content {
                let _ = writeln!(
                    out,
                    "  content: {:?} vs {:?}",
                    loro_block.content, ref_block.content
                );
            }
            if loro_block.parent_id != ref_block.parent_id {
                let _ = writeln!(
                    out,
                    "  parent_id: {} vs {}",
                    loro_block.parent_id, ref_block.parent_id
                );
            }
            if loro_block.content_type != ref_block.content_type {
                let _ = writeln!(
                    out,
                    "  content_type: {:?} vs {:?}",
                    loro_block.content_type, ref_block.content_type
                );
            }
            if loro_block.properties != ref_block.properties {
                let _ = writeln!(
                    out,
                    "  properties: {:?} vs {:?}",
                    loro_block.properties, ref_block.properties
                );
            }
        }
    }

    out
}
