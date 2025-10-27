//! Loro → Org-mode rendering
//!
//! Converts Loro document blocks to org-mode format using Block with OrgBlockExt.

use crate::models::{OrgBlockExt, ToOrg};
use holon::sync::Document;
use holon_api::block::Block;
use holon_api::types::ContentType;
use holon_api::Value;
use std::collections::HashMap;
use std::path::Path;

/// Render a Loro document (represented as blocks) to org-mode format.
///
/// Takes a list of blocks in tree order and converts them to org-mode text.
pub struct OrgRenderer;

impl OrgRenderer {
    /// Render a complete org document: header (#+TITLE, #+TODO) + blocks.
    ///
    /// This is THE SINGLE path for producing a complete org file from blocks.
    /// THE SINGLE path for producing a complete org file from blocks.
    pub fn render_document(
        doc: &Document,
        blocks: &[Block],
        file_path: &Path,
        file_id: &str,
    ) -> String {
        let mut result = doc.to_org();
        if !result.is_empty() && !result.ends_with('\n') {
            result.push('\n');
        }
        result.push_str(&Self::render_blocks(blocks, file_path, file_id));
        result
    }

    /// Render blocks to org-mode format.
    ///
    /// # Arguments
    /// * `blocks` - Blocks in tree order (parent before children)
    /// * `file_path` - Path to the org file (for OrgBlock metadata)
    /// * `file_id` - ID of the org file
    ///
    /// # Returns
    /// Org-mode formatted string
    pub fn render_blocks(blocks: &[Block], file_path: &Path, file_id: &str) -> String {
        let mut result = String::new();

        // Build a map of block ID to block for quick lookup
        let block_map: HashMap<&str, &Block> = blocks.iter().map(|b| (b.id.as_str(), b)).collect();

        // Find content root blocks - blocks that should be rendered as top-level content.
        // These are blocks whose parent is a document URI.
        // Sort by sequence to produce deterministic output regardless of input order
        // (blocks may arrive in arbitrary order from Loro's HashMap).
        let mut root_blocks: Vec<&Block> = blocks
            .iter()
            .filter(|b| b.parent_id.is_document())
            .collect();
        root_blocks.sort_by(|a, b| {
            a.sequence()
                .cmp(&b.sequence())
                .then_with(|| a.id.as_str().cmp(b.id.as_str()))
        });

        // Render each root block and its children recursively
        for root_block in root_blocks {
            Self::render_block_tree(root_block, &block_map, file_path, file_id, &mut result, 0);
        }

        result
    }

    /// Render a block and its children recursively.
    fn render_block_tree(
        block: &Block,
        block_map: &HashMap<&str, &Block>,
        _file_path: &Path,
        _file_id: &str,
        result: &mut String,
        depth: usize,
    ) {
        // Prepare block for org rendering - transfer Loro properties to org_props format
        let mut prepared_block = block.clone();
        Self::prepare_block_for_org(&mut prepared_block, depth);

        // Render using Block::to_org() which guarantees trailing newline
        result.push_str(&prepared_block.to_org());

        // Render children (find blocks where parent_id matches this block's id)
        // Source blocks must come BEFORE text children (sub-headings) so that
        // when the org file is re-parsed, the source block is assigned to this
        // parent heading, not to the first sub-heading that follows it.
        // Within each group, sort by sequence for deterministic output regardless
        // of input order (blocks arrive in arbitrary order from Loro's HashMap).
        let mut child_blocks: Vec<_> = block_map
            .values()
            .filter(|b| b.parent_id == block.id)
            .collect();
        child_blocks.sort_by(|a, b| {
            let a_type: i64 = if a.content_type == ContentType::Source {
                0
            } else {
                1
            };
            let b_type: i64 = if b.content_type == ContentType::Source {
                0
            } else {
                1
            };
            a_type
                .cmp(&b_type)
                .then_with(|| a.sequence().cmp(&b.sequence()))
                .then_with(|| a.id.as_str().cmp(b.id.as_str()))
        });
        for child_block in child_blocks {
            Self::render_block_tree(
                child_block,
                block_map,
                _file_path,
                _file_id,
                result,
                depth + 1,
            );
        }
    }

    /// Prepare a block for org rendering by transferring Loro properties to org_props format.
    fn prepare_block_for_org(block: &mut Block, depth: usize) {
        let properties = block.properties_map();

        // Set level from depth (level = depth + 1)
        block.set_level((depth + 1) as i64);

        // Transfer TODO to task_state if not already set
        if block.task_state().is_none() {
            if let Some(todo) = properties.get("TODO").and_then(|v| v.as_string()) {
                block.set_task_state(Some(holon_api::TaskState::from_keyword(&todo)));
            }
        }

        // Transfer PRIORITY to priority if not already set
        if block.priority().is_none() {
            if let Some(priority_val) = properties.get("PRIORITY") {
                let priority = match priority_val {
                    Value::String(s) => holon_api::Priority::from_letter(s).ok(),
                    Value::Integer(n) => holon_api::Priority::from_int(*n as i32).ok(),
                    Value::Float(f) => holon_api::Priority::from_int(*f as i32).ok(),
                    _ => None,
                };
                if let Some(p) = priority {
                    block.set_priority(Some(p));
                }
            }
        }

        // Transfer TAGS to tags if not already set
        if block.tags().is_empty() {
            if let Some(tags) = properties.get("TAGS").and_then(|v| v.as_string()) {
                block.set_tags(holon_api::Tags::from_csv(tags));
            }
        }

        // Transfer SCHEDULED if not already set
        if block.scheduled().is_none() {
            if let Some(sched) = properties.get("SCHEDULED").and_then(|v| v.as_string()) {
                match holon_api::types::Timestamp::parse(&sched) {
                    Ok(ts) => block.set_scheduled(Some(ts)),
                    Err(e) => {
                        tracing::warn!("Ignoring unparseable SCHEDULED property {sched:?}: {e}")
                    }
                }
            }
        }

        // Transfer DEADLINE if not already set
        if block.deadline().is_none() {
            if let Some(dead) = properties.get("DEADLINE").and_then(|v| v.as_string()) {
                match holon_api::types::Timestamp::parse(&dead) {
                    Ok(ts) => block.set_deadline(Some(ts)),
                    Err(e) => {
                        tracing::warn!("Ignoring unparseable DEADLINE property {dead:?}: {e}")
                    }
                }
            }
        }

        // Reconstruct org_properties JSON when missing (after SQL round-trip,
        // flat properties like "ID" exist but the "org_properties" JSON key doesn't).
        // to_org() renders the :PROPERTIES: drawer exclusively from org_properties().
        if block.org_properties().is_none() {
            let id = properties
                .get("ID")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
                .unwrap_or_else(|| block.id.id().to_string());

            let mut org_props = serde_json::Map::new();
            org_props.insert("ID".to_string(), serde_json::Value::String(id));
            for (k, v) in block.drawer_properties() {
                org_props.insert(k, serde_json::Value::String(v));
            }
            let json = serde_json::to_string(&org_props)
                .expect("drawer properties must serialize to JSON");
            block.set_org_properties(Some(json));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_api::types::SourceLanguage;
    use holon_api::EntityUri;

    const TEST_DOC: &str = "doc:/test/file.org";

    fn test_doc_uri() -> EntityUri {
        EntityUri::doc("/test/file.org")
    }

    fn test_source_block(id: &str, parent_id: &str, lang: &str, content: &str, seq: i64) -> Block {
        use holon_orgmode_models::OrgBlockExt;
        let mut b = Block {
            id: EntityUri::block(id),
            parent_id: EntityUri::block(parent_id),
            document_id: test_doc_uri(),
            content: content.to_string(),
            content_type: ContentType::Source,
            source_language: Some(lang.parse::<SourceLanguage>().unwrap()),
            source_name: None,
            properties: HashMap::new(),
            created_at: 0,
            updated_at: 0,
        };
        b.set_sequence(seq);
        b
    }

    use crate::models as holon_orgmode_models;

    #[test]
    fn test_render_simple_block() {
        let mut block = Block::new_text(
            EntityUri::parse("local://test-uuid").unwrap(),
            test_doc_uri(),
            test_doc_uri(),
            "Test Title\nBody content here",
        );
        block.set_property("ID", Value::String("local://test-uuid".to_string()));

        let file_path = Path::new("/test/file.org");
        let org_text = OrgRenderer::render_blocks(&[block], file_path, TEST_DOC);

        assert!(org_text.contains("* Test Title"));
        assert!(org_text.contains("Body content here"));
        assert!(org_text.contains(":ID: local://test-uuid"));
    }

    #[test]
    fn test_render_block_with_todo_and_priority() {
        let mut block = Block::new_text(
            EntityUri::block("test-id"),
            test_doc_uri(),
            test_doc_uri(),
            "Task headline",
        );
        block.set_property("ID", Value::String("test-id".to_string()));
        block.set_property("TODO", Value::String("TODO".to_string()));
        block.set_property("PRIORITY", Value::String("A".to_string()));

        let file_path = Path::new("/test/file.org");
        let org_text = OrgRenderer::render_blocks(&[block], file_path, TEST_DOC);

        assert!(org_text.contains("* TODO [#A] Task headline"));
    }

    #[test]
    fn test_source_blocks_render_before_child_headlines() {
        let doc = test_doc_uri();

        let mut parent = Block::new_text(
            EntityUri::block("parent-id"),
            doc.clone(),
            doc.clone(),
            "Parent Heading",
        );
        parent.set_property("ID", Value::String("parent-id".to_string()));

        let mut child_heading = Block::new_text(
            EntityUri::block("child-heading-id"),
            EntityUri::block("parent-id"),
            doc.clone(),
            "Child Heading",
        );
        child_heading.set_property("ID", Value::String("child-heading-id".to_string()));

        let source_block =
            test_source_block("src-id", "parent-id", "holon_prql", "from tasks\n", 1);

        let file_path = Path::new("/test/file.org");
        let blocks = vec![parent, child_heading, source_block];
        let org_text = OrgRenderer::render_blocks(&blocks, file_path, TEST_DOC);

        let src_pos = org_text
            .find("#+BEGIN_SRC")
            .expect("source block must be present");
        let child_pos = org_text
            .find("** Child Heading")
            .expect("child heading must be present");

        assert!(
            src_pos < child_pos,
            "Source block must render BEFORE child heading.\nOutput:\n{}",
            org_text
        );
    }

    #[test]
    fn test_multiple_source_blocks_all_before_children() {
        let doc = test_doc_uri();

        let mut parent = Block::new_text(
            EntityUri::block("parent-id"),
            doc.clone(),
            doc.clone(),
            "Parent",
        );
        parent.set_property("ID", Value::String("parent-id".to_string()));

        let src1 = test_source_block("src1", "parent-id", "holon_sql", "SELECT 1;\n", 1);
        let src2 = test_source_block("src2", "parent-id", "holon_prql", "from users\n", 2);

        let mut child = Block::new_text(
            EntityUri::block("child-id"),
            EntityUri::block("parent-id"),
            doc.clone(),
            "Child",
        );
        child.set_property("ID", Value::String("child-id".to_string()));

        let file_path = Path::new("/test/file.org");
        let blocks = vec![parent, child, src1, src2];
        let org_text = OrgRenderer::render_blocks(&blocks, file_path, TEST_DOC);

        let src1_pos = org_text
            .find("#+BEGIN_SRC holon_sql")
            .expect("holon_sql block");
        let src2_pos = org_text
            .find("#+BEGIN_SRC holon_prql")
            .expect("holon_prql block");
        let child_pos = org_text.find("** Child").expect("child heading");

        assert!(
            src1_pos < child_pos && src2_pos < child_pos,
            "All source blocks must render before child heading.\nOutput:\n{}",
            org_text
        );
    }

    #[test]
    fn test_source_block_ordering_with_interleaved_input() {
        let doc = test_doc_uri();

        let mut parent = Block::new_text(EntityUri::block("p"), doc.clone(), doc.clone(), "Root");
        parent.set_property("ID", Value::String("p".to_string()));

        let mut text_child = Block::new_text(
            EntityUri::block("t1"),
            EntityUri::block("p"),
            doc.clone(),
            "Sub Heading",
        );
        text_child.set_property("ID", Value::String("t1".to_string()));

        let src_child = test_source_block("s1", "p", "python", "print('hi')\n", 10);

        // Deliberately put text_child before src_child in the input vec
        let file_path = Path::new("/test/file.org");
        let blocks = vec![parent, text_child, src_child];
        let org_text = OrgRenderer::render_blocks(&blocks, file_path, TEST_DOC);

        let src_pos = org_text.find("#+BEGIN_SRC python").expect("source block");
        let sub_pos = org_text.find("** Sub Heading").expect("sub heading");

        assert!(
            src_pos < sub_pos,
            "Source block must come first regardless of input order.\nOutput:\n{}",
            org_text
        );
    }
}
