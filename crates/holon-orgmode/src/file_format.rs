//! `OrgFormatAdapter` — implements `holon_core::FileFormatAdapter` for `.org`.
//!
//! Stateless wrapper: delegates to `holon_org_format::parser::parse_org_file`
//! and `holon_org_format::org_renderer::OrgRenderer` so the sync controller
//! can call parse/render through the trait without knowing the format.

use std::path::Path;

use anyhow::Result;
use holon_api::block::Block;
use holon_api::EntityUri;
use holon_api::StorageEntity;
use holon_core::file_format::FileFormatAdapter;
use holon_core::file_format::FileFormatParseResult;

use crate::block_params::build_block_params;
use crate::models::OrgBlockExt;
use crate::models::OrgDocumentExt;
use crate::org_renderer::OrgRenderer;
use crate::parser::parse_doc_id;
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

    fn doc_id_from_content(&self, content: &str) -> Option<String> {
        parse_doc_id(content)
    }

    fn build_block_params(
        &self,
        block: &Block,
        parent_id: &EntityUri,
        document_uri: &EntityUri,
    ) -> StorageEntity {
        build_block_params(block, parent_id, document_uri)
    }

    fn content_differs(&self, a: &Block, b: &Block) -> bool {
        a.content != b.content
            || a.parent_id != b.parent_id
            || a.content_type != b.content_type
            || a.source_language != b.source_language
            || a.source_name != b.source_name
            || a.task_state() != b.task_state()
            || a.priority() != b.priority()
            || a.tags() != b.tags()
            || a.scheduled() != b.scheduled()
            || a.deadline() != b.deadline()
            || a.drawer_properties() != b.drawer_properties()
            || a.sequence() != b.sequence()
        // Sibling order is no longer a per-block field (ADR 0005): it is
        // derived from document position and applied via `place_all`,
        // so it is not part of this content-equivalence check.
    }

    fn sync_document_metadata(&self, parsed: &Block, persisted: &mut Block) -> bool {
        let parsed_kws = parsed.todo_keywords();
        if parsed_kws != persisted.todo_keywords() {
            persisted.set_todo_keywords(parsed_kws);
            true
        } else {
            false
        }
    }

    fn check_writeback_lossless(
        &self,
        path: &Path,
        source: &str,
        rendered: &str,
        sibling_renders: &[(&Path, &str)],
        sanctioned_removals: &std::collections::HashSet<String>,
        root: &Path,
    ) -> Result<()> {
        let mut surviving =
            crate::writeback_guard::SurvivingProjection::from_rendered(path, rendered, root)?;
        for (sibling_path, sibling_rendered) in sibling_renders {
            surviving.union_rendered(sibling_path, sibling_rendered, root)?;
        }
        crate::writeback_guard::ensure_ingest_lossless(
            path,
            source,
            &surviving,
            sanctioned_removals,
            root,
        )
    }

    fn writeback_drops(
        &self,
        path: &Path,
        source: &str,
        rendered: &str,
        sibling_renders: &[(&Path, &str)],
        sanctioned_removals: &std::collections::HashSet<String>,
        root: &Path,
    ) -> Result<holon_core::file_format::WritebackDropVerdict> {
        let mut surviving =
            crate::writeback_guard::SurvivingProjection::from_rendered(path, rendered, root)?;
        for (sibling_path, sibling_rendered) in sibling_renders {
            surviving.union_rendered(sibling_path, sibling_rendered, root)?;
        }
        let drops = crate::writeback_guard::writeback_drops(
            path,
            source,
            &surviving,
            sanctioned_removals,
            root,
        )?;
        Ok(holon_core::file_format::WritebackDropVerdict {
            dropped: drops.dropped,
            source_block_count: drops.source_block_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

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
