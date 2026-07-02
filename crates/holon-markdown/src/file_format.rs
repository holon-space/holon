//! `MarkdownFormatAdapter` — implements `holon_core::FileFormatAdapter`
//! for `.md` / `.markdown`.
//!
//! Wraps `parse_markdown_file` and `MarkdownRenderer`, analogous to
//! `holon_orgmode::OrgFormatAdapter`. Unlike the org adapter it carries a
//! [`MarkdownDialect`] describing the vault's flavor — the trait methods keep
//! their format-neutral signatures, and the dialect rides on the adapter so a
//! `FileSyncController::with_format(...)` can host a full Obsidian vault, a
//! plain CommonMark tree, or any orthogonal mix in between by choosing a
//! preset.

use anyhow::Result;
use holon_api::block::Block;
use holon_api::{EntityUri, StorageEntity};
use holon_core::file_format::{FileFormatAdapter, FileFormatParseResult};
use std::path::Path;

use crate::dialect::MarkdownDialect;
use crate::parser::parse_markdown_file;
use crate::renderer::MarkdownRenderer;

pub struct MarkdownFormatAdapter {
    dialect: MarkdownDialect,
}

impl MarkdownFormatAdapter {
    /// Full Obsidian flavor — the default and the crate's historical behavior.
    pub fn new() -> Self {
        Self::obsidian()
    }

    /// Adapter for an explicit dialect.
    pub fn with_dialect(dialect: MarkdownDialect) -> Self {
        Self { dialect }
    }

    /// Full Obsidian flavor — every extension on.
    pub fn obsidian() -> Self {
        Self::with_dialect(MarkdownDialect::obsidian())
    }

    /// Plain CommonMark — every Obsidian extension off.
    pub fn commonmark() -> Self {
        Self::with_dialect(MarkdownDialect::commonmark())
    }

    /// The dialect this adapter parses and renders with.
    pub fn dialect(&self) -> &MarkdownDialect {
        &self.dialect
    }
}

impl Default for MarkdownFormatAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl FileFormatAdapter for MarkdownFormatAdapter {
    fn extensions(&self) -> &'static [&'static str] {
        &["md", "markdown"]
    }

    fn parse(
        &self,
        path: &Path,
        content: &str,
        parent_dir_id: &EntityUri,
        root: &Path,
    ) -> Result<FileFormatParseResult> {
        let r = parse_markdown_file(path, content, parent_dir_id, root, &self.dialect)?;
        Ok(FileFormatParseResult {
            document: r.document,
            blocks: r.blocks,
            blocks_needing_ids: r.blocks_needing_ids,
        })
    }

    fn render_document(
        &self,
        document: &Block,
        blocks: &[Block],
        file_path: &Path,
        file_id: &EntityUri,
    ) -> String {
        // The shared `FileFormatAdapter` trait returns `String` because the
        // live org path never fails to render. Markdown *can* fail (an
        // out-of-charset id has no round-trip-safe marker), but widening the
        // trait to `Result` purely for this latent, zero-dependents adapter
        // would ripple through the live org renderer + file-sync controller —
        // machinery for a path that is not yet wired. So we surface the error
        // loudly here instead of swallowing it. When markdown graduates into
        // file-sync, widen the trait to `Result` and drop this panic.
        MarkdownRenderer::new(self.dialect.clone())
            .render_document(document, blocks, file_path, file_id)
            .unwrap_or_else(|e| panic!("markdown adapter cannot render document {file_id:?}: {e}"))
    }

    fn render_blocks(&self, blocks: &[Block], file_path: &Path, file_id: &EntityUri) -> String {
        MarkdownRenderer::new(self.dialect.clone())
            .render_blocks(blocks, file_path, file_id)
            .unwrap_or_else(|e| {
                panic!("markdown adapter cannot render blocks under {file_id:?}: {e}")
            })
    }

    // The write-path / identity seam below is exercised only when a
    // `FileSyncController` drives a markdown vault back to a backend. That
    // wiring does not exist yet (markdown is read/render-complete; bidirectional
    // sync is future work), and markdown has no established id-in-content or
    // op-param convention. Fail loud rather than fake a convention: an
    // implementor wiring markdown sync gets a precise pointer here.

    fn doc_id_from_content(&self, _: &str) -> Option<String> {
        unimplemented!(
            "MarkdownFormatAdapter::doc_id_from_content — markdown vault write-sync \
             is not wired; define the frontmatter id convention before enabling it"
        )
    }

    fn build_block_params(&self, _: &Block, _: &EntityUri, _: &EntityUri) -> StorageEntity {
        unimplemented!(
            "MarkdownFormatAdapter::build_block_params — markdown vault write-sync \
             is not wired; define the op-param mapping before enabling it"
        )
    }

    fn content_differs(&self, _: &Block, _: &Block) -> bool {
        unimplemented!(
            "MarkdownFormatAdapter::content_differs — markdown vault write-sync \
             is not wired; define content-equivalence before enabling it"
        )
    }

    fn sync_document_metadata(&self, _: &Block, _: &mut Block) -> bool {
        unimplemented!(
            "MarkdownFormatAdapter::sync_document_metadata — markdown vault write-sync \
             is not wired; markdown has no #+TODO:-style header metadata to reconcile"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn extensions_cover_md_and_markdown() {
        let a = MarkdownFormatAdapter::new();
        assert_eq!(a.extensions(), &["md", "markdown"]);
    }

    #[test]
    fn default_adapter_is_obsidian() {
        assert_eq!(
            MarkdownFormatAdapter::default().dialect(),
            &MarkdownDialect::obsidian()
        );
    }

    #[test]
    fn parse_via_adapter_matches_direct_call() {
        let adapter = MarkdownFormatAdapter::new();
        let path = PathBuf::from("/tmp/note.md");
        let root = PathBuf::from("/tmp");
        let parent = EntityUri::no_parent();
        let content = "---\ntitle: Hi\n---\n# A ^aa\n\nbody\n\n## B ^bb\n";

        let via_adapter = adapter.parse(&path, content, &parent, &root).unwrap();
        let via_direct =
            parse_markdown_file(&path, content, &parent, &root, adapter.dialect()).unwrap();

        assert_eq!(via_adapter.blocks.len(), via_direct.blocks.len());
        assert_eq!(via_adapter.document.id, via_direct.document.id);
        assert_eq!(
            via_adapter.blocks_needing_ids,
            via_direct.blocks_needing_ids
        );
    }

    #[test]
    fn render_via_adapter_matches_direct_call() {
        let adapter = MarkdownFormatAdapter::new();
        let path = PathBuf::from("/tmp/note.md");
        let root = PathBuf::from("/tmp");
        let parent = EntityUri::no_parent();
        let content = "# A ^aa\n\nbody\n";
        let parsed = adapter.parse(&path, content, &parent, &root).unwrap();
        let via_adapter = adapter.render_blocks(&parsed.blocks, &path, &parsed.document.id);
        let via_direct = MarkdownRenderer::new(MarkdownDialect::obsidian())
            .render_blocks(&parsed.blocks, &path, &parsed.document.id)
            .unwrap();
        assert_eq!(via_adapter, via_direct);
    }

    #[test]
    fn round_trip_preserves_heading_structure_and_block_ids() {
        let adapter = MarkdownFormatAdapter::new();
        let path = PathBuf::from("/tmp/note.md");
        let root = PathBuf::from("/tmp");
        let parent = EntityUri::no_parent();
        let original =
            "---\ntitle: Round Trip\n---\n\n# First ^aa\n\nfirst body\n\n## Sub ^bb\n\nsub body\n";

        let parsed = adapter.parse(&path, original, &parent, &root).unwrap();
        let rendered =
            adapter.render_document(&parsed.document, &parsed.blocks, &path, &parsed.document.id);
        let reparsed = adapter.parse(&path, &rendered, &parent, &root).unwrap();

        assert_eq!(reparsed.blocks.len(), 2);
        assert_eq!(reparsed.blocks[0].id.id(), "aa");
        assert_eq!(reparsed.blocks[1].id.id(), "bb");
        assert_eq!(reparsed.blocks[1].parent_id, reparsed.blocks[0].id);
        // No new IDs needed on the second pass — they stuck.
        assert!(reparsed.blocks_needing_ids.is_empty());
    }

    #[test]
    fn round_trip_preserves_code_fence_as_source_child() {
        let adapter = MarkdownFormatAdapter::new();
        let path = PathBuf::from("/tmp/note.md");
        let root = PathBuf::from("/tmp");
        let parent = EntityUri::no_parent();
        let original = "# H ^aa\n\n```python\nprint(1)\n```\n";

        let parsed = adapter.parse(&path, original, &parent, &root).unwrap();
        let rendered =
            adapter.render_document(&parsed.document, &parsed.blocks, &path, &parsed.document.id);
        let reparsed = adapter.parse(&path, &rendered, &parent, &root).unwrap();

        let source = reparsed
            .blocks
            .iter()
            .find(|b| matches!(b.content_type, holon_api::types::ContentType::Source))
            .expect("source child survives round-trip");
        assert!(source.content.contains("print(1)"));
        assert_eq!(
            source
                .source_language
                .as_ref()
                .map(|l| l.to_string())
                .as_deref(),
            Some("python")
        );
    }
}
