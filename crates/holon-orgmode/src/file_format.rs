//! `OrgFormatAdapter` — implements `holon_core::FileFormatAdapter` for `.org`.
//!
//! Stateless wrapper: delegates to `holon_org_format::parser::parse_org_file`
//! and `holon_org_format::org_renderer::OrgRenderer` so the sync controller
//! can call parse/render through the trait without knowing the format.

use anyhow::Result;
use holon_api::block::Block;
use holon_api::EntityUri;
use holon_core::file_format::{FileFormatAdapter, FileFormatParseResult};
use std::path::Path;

use crate::org_renderer::OrgRenderer;
use crate::parser::parse_org_file;

pub struct OrgFormatAdapter;

impl OrgFormatAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for OrgFormatAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl FileFormatAdapter for OrgFormatAdapter {
    fn extensions(&self) -> &'static [&'static str] {
        &["org"]
    }

    fn parse(
        &self,
        path: &Path,
        content: &str,
        parent_dir_id: &EntityUri,
        root: &Path,
    ) -> Result<FileFormatParseResult> {
        let result = parse_org_file(path, content, parent_dir_id, root)?;
        Ok(FileFormatParseResult {
            document: result.document,
            blocks: result.blocks,
            blocks_needing_ids: result.headlines_needing_ids,
        })
    }

    fn render_document(
        &self,
        document: &Block,
        blocks: &[Block],
        file_path: &Path,
        file_id: &EntityUri,
    ) -> String {
        OrgRenderer::render_document(document, blocks, file_path, file_id)
    }

    fn render_blocks(&self, blocks: &[Block], file_path: &Path, file_id: &EntityUri) -> String {
        OrgRenderer::render_entitys(blocks, file_path, file_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_returns_same_document_and_blocks_as_underlying_parser() {
        let adapter = OrgFormatAdapter::new();
        let path = PathBuf::from("/tmp/test.org");
        let root = PathBuf::from("/tmp");
        let parent = EntityUri::no_parent();
        let content = "* Hello World\n:PROPERTIES:\n:ID: block-1\n:END:\n";

        let via_adapter = adapter.parse(&path, content, &parent, &root).unwrap();
        let via_direct = parse_org_file(&path, content, &parent, &root).unwrap();

        assert_eq!(via_adapter.blocks.len(), via_direct.blocks.len());
        assert_eq!(via_adapter.document.id, via_direct.document.id);
        assert_eq!(
            via_adapter.blocks_needing_ids,
            via_direct.headlines_needing_ids
        );
    }

    #[test]
    fn render_blocks_matches_underlying_renderer() {
        let adapter = OrgFormatAdapter::new();
        let path = PathBuf::from("/tmp/test.org");
        let root = PathBuf::from("/tmp");
        let parent = EntityUri::no_parent();
        let content = "* Hello World\n:PROPERTIES:\n:ID: block-1\n:END:\n";

        let parsed = adapter.parse(&path, content, &parent, &root).unwrap();
        let via_adapter = adapter.render_blocks(&parsed.blocks, &path, &parsed.document.id);
        let via_direct = OrgRenderer::render_entitys(&parsed.blocks, &path, &parsed.document.id);
        assert_eq!(via_adapter, via_direct);
    }

    #[test]
    fn extensions_returns_org() {
        let adapter = OrgFormatAdapter::new();
        assert_eq!(adapter.extensions(), &["org"]);
    }
}
