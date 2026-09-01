//! The org write-back render, as one reusable production service.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use holon_api::Block;
use holon_api::EntityUri;
use holon_core::FileFormatAdapter;
use holon_core::FormatRegistry;
use holon_core::WriteTier;

use crate::sync_ports::BlockReader;
use crate::sync_ports::DocumentManager;

/// Turns a document's blocks into the exact file text write-back puts on disk.
///
/// [`FileSyncController`](crate::FileSyncController) renders through this on
/// every write-back; inspection callers (the `render_org` MCP tool) render
/// through the same instance, so "what would write-back produce" cannot drift
/// from what it actually produces.
///
/// Marks are rendered VERBATIM: `[[Some Page]]` reaches disk as the user
/// authored it even once the `block_links` junction resolves it. Link
/// resolution is a query index, and the id-rewrite belongs to navigate
/// (`docs/Explanation/DESIGN_LINKS.md`), so file bytes never depend on
/// resolution state — nor on which read produced the values.
pub struct WritebackRenderer {
    block_reader: Arc<dyn BlockReader>,
    doc_manager: Arc<dyn DocumentManager>,
    formats: Arc<FormatRegistry>,
}

impl WritebackRenderer {
    pub fn new(
        block_reader: Arc<dyn BlockReader>,
        doc_manager: Arc<dyn DocumentManager>,
        formats: Arc<FormatRegistry>,
    ) -> Self {
        Self {
            block_reader,
            doc_manager,
            formats,
        }
    }

    /// The adapter for `path`, refused unless its format can be written back.
    ///
    /// The controller gates on [`WriteTier`] before it ever renders, so a
    /// read-only format reaching here means a write path bypassed that gate.
    /// It is an `Err` and not the adapter's own `panic!` because a render
    /// task that aborts discloses nothing to the user whose file it was about
    /// to overwrite.
    fn writable_adapter(&self, path: &Path) -> Result<Arc<dyn FileFormatAdapter>> {
        let adapter = self.formats.require(path)?;
        if adapter.write_tier() == WriteTier::ReadOnly {
            anyhow::bail!(
                "write-back render REFUSED for {}: its format is read-only (authoritative input \
                 only) and ships no renderer, so writing a reconstructed file over it would be \
                 loss. Reaching this render means a write path skipped the controller's \
                 write-tier gate.",
                path.display()
            );
        }
        Ok(adapter)
    }

    /// `doc_id`'s blocks from the write authority, in document order.
    pub async fn read_blocks(&self, doc_id: &EntityUri) -> Result<Vec<Block>> {
        self.block_reader.get_blocks(doc_id).await
    }

    /// Render `blocks` — already in authoritative document order — as
    /// `doc_id`'s file text.
    ///
    /// A known document renders with its header (`#+TITLE:`, `#+ID:`); an
    /// unknown one renders body-only.
    pub async fn render_blocks(
        &self,
        doc_id: &EntityUri,
        path: &Path,
        blocks: &[Block],
    ) -> Result<String> {
        let rendered = match self.doc_manager.get_by_id(doc_id).await? {
            Some(doc) => self.render_with_document_block(&doc, blocks, path)?,
            None => self.render_body_raw(doc_id, path, blocks)?,
        };
        assert_rendered(doc_id, blocks, &rendered);
        Ok(rendered)
    }

    /// Render with an explicitly supplied document block — for a caller that
    /// already holds the page's own doc-root and must not re-look it up.
    pub async fn render_document_block(
        &self,
        document: &Block,
        blocks: &[Block],
        path: &Path,
    ) -> Result<String> {
        self.render_with_document_block(document, blocks, path)
    }

    /// The authoritative full render: read `doc_id`'s blocks from the write
    /// authority, then render them.
    pub async fn render_document(&self, doc_id: &EntityUri, path: &Path) -> Result<String> {
        let blocks = self.read_blocks(doc_id).await?;
        self.render_blocks(doc_id, path, &blocks).await
    }

    /// Render with an explicitly supplied document block, so a caller reading
    /// blocks from a non-authoritative store (the Loro tree) can render that
    /// store's header rather than the write authority's.
    ///
    /// The document block's own id is the root parent reference, which may
    /// differ from the id used to look it up (`file:` vs `block:` schemes).
    fn render_with_document_block(
        &self,
        document: &Block,
        blocks: &[Block],
        path: &Path,
    ) -> Result<String> {
        Ok(self
            .writable_adapter(path)?
            .render_document(document, blocks, path, &document.id))
    }

    /// Render the body alone — no document header.
    pub async fn render_body(
        &self,
        doc_id: &EntityUri,
        path: &Path,
        blocks: &[Block],
    ) -> Result<String> {
        self.render_body_raw(doc_id, path, blocks)
    }

    fn render_body_raw(&self, doc_id: &EntityUri, path: &Path, blocks: &[Block]) -> Result<String> {
        Ok(self
            .writable_adapter(path)?
            .render_blocks(blocks, path, doc_id))
    }
}

fn assert_rendered(doc_id: &EntityUri, blocks: &[Block], rendered: &str) {
    assert!(
        blocks.is_empty() || !rendered.trim().is_empty(),
        "[WritebackRenderer] {} blocks for doc {} but render is empty!\nBlocks: {:?}",
        blocks.len(),
        doc_id,
        blocks
            .iter()
            .map(|b| format!(
                "{{id={}, parent_id={}, content_type={}}}",
                b.id, b.parent_id, b.content_type
            ))
            .collect::<Vec<_>>()
    );
}
