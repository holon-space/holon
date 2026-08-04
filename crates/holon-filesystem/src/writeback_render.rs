//! The org write-back render, as one reusable production service.

use std::borrow::Cow;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use holon_api::Block;
use holon_api::EntityRef;
use holon_api::EntityUri;
use holon_api::InlineMark;
use holon_core::FileFormatAdapter;

use crate::sync_ports::BlockReader;
use crate::sync_ports::DocumentManager;

/// Turns a document's blocks into the exact file text write-back puts on disk.
///
/// [`FileSyncController`](crate::FileSyncController) renders through this on
/// every write-back; inspection callers (the `render_org` MCP tool) render
/// through the same instance, so "what would write-back produce" cannot drift
/// from what it actually produces.
pub struct WritebackRenderer {
    block_reader: Arc<dyn BlockReader>,
    doc_manager: Arc<dyn DocumentManager>,
    format: Arc<dyn FileFormatAdapter>,
}

impl WritebackRenderer {
    pub fn new(
        block_reader: Arc<dyn BlockReader>,
        doc_manager: Arc<dyn DocumentManager>,
        format: Arc<dyn FileFormatAdapter>,
    ) -> Self {
        Self {
            block_reader,
            doc_manager,
            format,
        }
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
        let blocks = self.with_resolved_links(blocks).await?;
        let rendered = match self.doc_manager.get_by_id(doc_id).await? {
            Some(doc) => self.render_with_document_block(&doc, &blocks, path),
            None => self.render_body_raw(doc_id, path, &blocks),
        };
        assert_rendered(doc_id, &blocks, &rendered);
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
        let blocks = self.with_resolved_links(blocks).await?;
        Ok(self.render_with_document_block(document, &blocks, path))
    }

    /// Apply the render-time link-mark upgrade to the slice about to be
    /// rendered, so the output does not depend on which read produced the
    /// values.
    ///
    /// Borrows unless there is something to upgrade: the clone is paid only by
    /// documents that actually carry a dangling `Name` link.
    async fn with_resolved_links<'a>(&self, blocks: &'a [Block]) -> Result<Cow<'a, [Block]>> {
        if !blocks.iter().any(has_dangling_name_link) {
            return Ok(Cow::Borrowed(blocks));
        }
        let mut owned = blocks.to_vec();
        self.block_reader.resolve_link_marks(&mut owned).await?;
        Ok(Cow::Owned(owned))
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
    ///
    /// Private: it renders the slice VERBATIM. Every public entry resolves
    /// link marks first, and a caller reaching this directly would silently
    /// skip that step.
    fn render_with_document_block(
        &self,
        document: &Block,
        blocks: &[Block],
        path: &Path,
    ) -> String {
        self.format
            .render_document(document, blocks, path, &document.id)
    }

    /// Render the body alone — no document header.
    pub async fn render_body(
        &self,
        doc_id: &EntityUri,
        path: &Path,
        blocks: &[Block],
    ) -> Result<String> {
        let blocks = self.with_resolved_links(blocks).await?;
        Ok(self.render_body_raw(doc_id, path, &blocks))
    }

    /// Private for the same reason as [`Self::render_with_document_block`]:
    /// it renders the slice verbatim.
    fn render_body_raw(&self, doc_id: &EntityUri, path: &Path, blocks: &[Block]) -> String {
        self.format.render_blocks(blocks, path, doc_id)
    }
}

/// Whether this block carries a link mark still pointing at a bare name — the
/// only shape [`BlockReader::resolve_link_marks`] can upgrade.
fn has_dangling_name_link(block: &Block) -> bool {
    block.marks.as_ref().is_some_and(|marks| {
        marks.iter().any(|span| {
            matches!(
                &span.mark,
                InlineMark::Link {
                    target: EntityRef::Name { .. },
                    ..
                }
            )
        })
    })
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
