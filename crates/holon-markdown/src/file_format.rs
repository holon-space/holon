//! `MarkdownFormatAdapter` — implements `holon_core::FileFormatAdapter`
//! for `.md` / `.markdown`.
//!
//! Stateless wrapper around `parse_markdown_file` and `MarkdownRenderer`,
//! analogous to `holon_orgmode::OrgFormatAdapter`. Plug into
//! `OrgSyncController::with_format(...)` to drive an Obsidian-style vault
//! through the same controller used for org files.

use anyhow::Result;
use holon_api::block::Block;
use holon_api::EntityUri;
use holon_core::file_format::{FileFormatAdapter, FileFormatParseResult};
use std::path::Path;

use crate::parser::parse_markdown_file;
use crate::renderer::MarkdownRenderer;

pub struct MarkdownFormatAdapter;

impl MarkdownFormatAdapter {
    pub fn new() -> Self {
        Self
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
        let r = parse_markdown_file(path, content, parent_dir_id, root)?;
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
        MarkdownRenderer::render_document(document, blocks, file_path, file_id)
    }

    fn render_blocks(&self, blocks: &[Block], file_path: &Path, file_id: &EntityUri) -> String {
        MarkdownRenderer::render_blocks(blocks, file_path, file_id)
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
    fn parse_via_adapter_matches_direct_call() {
        let adapter = MarkdownFormatAdapter::new();
        let path = PathBuf::from("/tmp/note.md");
        let root = PathBuf::from("/tmp");
        let parent = EntityUri::no_parent();
        let content = "---\ntitle: Hi\n---\n# A ^aa\n\nbody\n\n## B ^bb\n";

        let via_adapter = adapter.parse(&path, content, &parent, &root).unwrap();
        let via_direct = parse_markdown_file(&path, content, &parent, &root).unwrap();

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
        let via_direct =
            MarkdownRenderer::render_blocks(&parsed.blocks, &path, &parsed.document.id);
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
