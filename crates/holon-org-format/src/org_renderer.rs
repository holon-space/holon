//! Loro → Org-mode rendering
//!
//! Converts Loro document blocks to org-mode format using Block with
//! OrgBlockExt.

use std::collections::HashMap;
use std::path::Path;

use holon_api::EntityUri;
use holon_api::Value;
use holon_api::block::Block;

use crate::models::OrgBlockExt;
use crate::models::ToOrg;
use crate::models::render_document_header;

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
        doc_block: &Block,
        blocks: &[Block],
        file_path: &Path,
        file_id: &EntityUri,
    ) -> String {
        let mut result = render_document_header(doc_block);
        if !result.is_empty() && !result.ends_with('\n') {
            result.push('\n');
        }
        // The doc-root's OWN body — the pre-first-headline text. Like a
        // headline, a doc-root stores `title\nbody` in its content; the title
        // went out as `#+TITLE:` above, so everything after the first line is
        // body that belongs on disk between the `#+` directives and the first
        // headline. Without this it is silently deleted on every write-back,
        // and a page promoted from a `:Page:`-tagged headline loses the whole
        // body it was carrying.
        let preamble = doc_block
            .content
            .split_once('\n')
            .map(|(_, rest)| rest)
            .unwrap_or("")
            .trim_matches('\n');
        if !preamble.is_empty() {
            result.push('\n');
            result.push_str(preamble);
            result.push('\n');
        }
        result.push_str(&Self::render_entitys(blocks, file_path, file_id));
        // Every org file ends with exactly one '\n'. Strip any trailing
        // whitespace/newlines and re-add one — keeps disk content stable across
        // render → parse → render so PBT round-trips converge to a fixed point.
        while matches!(
            result.chars().last(),
            Some('\n') | Some(' ') | Some('\t') | Some('\r')
        ) {
            result.pop();
        }
        result.push('\n');
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
    pub fn render_entitys(blocks: &[Block], _: &Path, file_id: &EntityUri) -> String {
        Self::render_walk(blocks, file_id, &|b: &Block| b.to_org())
    }

    /// Dense projection variant of [`Self::render_entitys`]: identical tree
    /// walk and projection invariants, but each headline's `:ID:` drawer
    /// scaffolding is compressed to a trailing `{#alias}` token via
    /// `alias_table` (projection-only — see [`crate::dense`]). Source/Image
    /// blocks keep their canonical form.
    pub fn render_entitys_dense(
        blocks: &[Block],
        file_id: &EntityUri,
        alias_table: &crate::dense::AliasTable,
        gap_ids: &std::collections::HashSet<String>,
    ) -> String {
        Self::render_walk(blocks, file_id, &|b: &Block| {
            crate::dense::to_org_dense(b, alias_table, gap_ids)
        })
    }

    fn render_walk<F: Fn(&Block) -> String>(
        blocks: &[Block],
        file_id: &EntityUri,
        render_block: &F,
    ) -> String {
        let mut result = String::new();

        // Sibling order is the caller's responsibility — `blocks` arrives in
        // authoritative order (the ordered read; ADR 0005). The renderer trusts
        // it and never re-derives order from a per-block key. Build a
        // parent→children index that preserves the input order.
        let mut children_by_parent: HashMap<&str, Vec<&Block>> = HashMap::new();
        let mut ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for b in blocks {
            ids.insert(b.id.as_str());
            children_by_parent
                .entry(b.parent_id.as_str())
                .or_default()
                .push(b);
        }

        // WP-F projection assertion (cheap, no extra pass beyond ids we already
        // built): every block's stated parent must be the file root or another
        // block in this set. Otherwise the block is a dangling orphan that this
        // renderer would silently drop (never reachable from the file roots).
        // A self-parented row (the filtered-out `sentinel:no_parent` FK anchor)
        // is excluded so it can never trip a false positive. This path returns
        // `String` (see the `FileFormat::render_document` trait), so a `Result`
        // is not available — per the fail-loud directive a `panic!` is used.
        let file_id_str = file_id.as_str();
        for b in blocks {
            let parent = b.parent_id.as_str();
            if parent == b.id.as_str() {
                continue; // self-parented FK-anchor sentinel; never a real
                // block
            }
            if parent != file_id_str && !ids.contains(parent) {
                panic!(
                    "{}",
                    holon_api::ProjectionInvariantViolated {
                        detail: format!(
                            "org render: block {} has dangling parent {} (not the file root {} \
                             and not in the {}-block set)",
                            b.id.as_str(),
                            parent,
                            file_id_str,
                            blocks.len()
                        ),
                    }
                );
            }
        }
        // The only re-ordering the renderer imposes is a content-type grouping:
        // Source/Image children render before Text children (sub-headings) so a
        // re-parse re-attaches the source to this heading, not the next one. The
        // sort is stable, so input order is preserved within each group.
        for kids in children_by_parent.values_mut() {
            kids.sort_by_key(|b| b.content_type.sibling_order_group());
        }

        let mut visited: std::collections::HashSet<&str> = std::collections::HashSet::new();
        if let Some(roots) = children_by_parent.get(file_id.as_str()) {
            for root_block in roots {
                Self::render_entity_tree(
                    root_block,
                    &children_by_parent,
                    &mut result,
                    0,
                    &mut visited,
                    render_block,
                );
            }
        }

        // WP-F projection assertion (free — `visited` is populated by the walk we
        // already performed): with dangling parents ruled out above, every real
        // block chains up to a file root, so every block must have been visited.
        // A block that was NOT visited is present with a present parent yet
        // unreachable from any root — the signature of a parent CYCLE (or a
        // disconnected component). Self-parented sentinel rows are skipped so
        // they cannot masquerade as a cycle. No `Result` on this path → `panic!`.
        for b in blocks {
            if b.parent_id.as_str() == b.id.as_str() {
                continue; // self-parented FK-anchor sentinel
            }
            if !visited.contains(b.id.as_str()) {
                panic!(
                    "{}",
                    holon_api::ProjectionInvariantViolated {
                        detail: format!(
                            "org render: block {} (parent {}) is unreachable from file root {} \
                             despite its parent being present — parent cycle or disconnected \
                             component",
                            b.id.as_str(),
                            b.parent_id.as_str(),
                            file_id.as_str()
                        ),
                    }
                );
            }
        }

        result
    }

    /// Render a block and its children recursively.
    fn render_entity_tree<'b, F: Fn(&Block) -> String>(
        block: &'b Block,
        children_by_parent: &HashMap<&'b str, Vec<&'b Block>>,
        result: &mut String,
        depth: usize,
        visited: &mut std::collections::HashSet<&'b str>,
        render_block: &F,
    ) {
        // Record reachability for the WP-F cycle/disconnected-component assertion
        // in `render_entitys` — free, we are already walking every reachable node.
        visited.insert(block.id.as_str());

        // Prepare block for org rendering - transfer Loro properties to org_props
        // format
        let mut prepared_block = block.clone();
        Self::prepare_block_for_org(&mut prepared_block, depth);

        // Render via the caller-supplied per-block renderer (canonical
        // `Block::to_org` or the dense token form). Both guarantee a trailing
        // newline.
        result.push_str(&render_block(&prepared_block));

        if let Some(kids) = children_by_parent.get(block.id.as_str()) {
            for child_block in kids {
                Self::render_entity_tree(
                    child_block,
                    children_by_parent,
                    result,
                    depth + 1,
                    visited,
                    render_block,
                );
            }
        }
    }

    /// Prepare a block for org rendering by transferring Loro properties to
    /// org_props format.
    fn prepare_block_for_org(block: &mut Block, depth: usize) {
        let properties = block.properties_map();

        // Set level from depth (level = depth + 1)
        block.set_level((depth + 1) as i64);

        // Transfer TODO to task_state if not already set
        if block.task_state().is_none() {
            if let Some(todo) = properties.get("TODO").and_then(|v| v.as_string()) {
                block.set_task_state(Some(holon_api::TaskState::from_keyword(todo)));
            }
        }

        // Transfer PRIORITY to priority if not already set
        if block.priority().is_none() {
            if let Some(priority_val) = properties.get("PRIORITY") {
                let priority = match priority_val {
                    // ALLOW(ok): boundary parse — None valid for missing priority
                    Value::String(s) => holon_api::Priority::from_letter(s).ok(),
                    Value::Integer(n) => holon_api::Priority::from_int(*n as i32).ok(), /* ALLOW(ok): boundary parse */
                    Value::Float(f) => holon_api::Priority::from_int(*f as i32).ok(), /* ALLOW(ok): boundary parse */
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
                match holon_api::types::Timestamp::parse(sched) {
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
                match holon_api::types::Timestamp::parse(dead) {
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

            // Sort drawer properties by key for deterministic output.
            // serde_json::Map uses IndexMap (preserve_order feature is enabled
            // by a transitive dependency), so insertion order matters.
            let mut drawer_props: Vec<_> = block.drawer_properties().into_iter().collect();
            drawer_props.sort_by(|(a, _), (b, _)| a.cmp(b));

            let mut org_props = serde_json::Map::new();
            org_props.insert("ID".to_string(), serde_json::Value::String(id));
            for (k, v) in drawer_props {
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
    use holon_api::EntityUri;
    use holon_api::types::ContentType;
    use holon_api::types::SourceLanguage;

    use super::*;

    fn test_doc_uri() -> EntityUri {
        EntityUri::file("/test/file.org")
    }

    fn test_source_block(id: &str, parent_id: &str, lang: &str, content: &str, seq: i64) -> Block {
        use holon_orgmode_models::OrgBlockExt;
        let mut b = Block {
            id: EntityUri::block(id),
            parent_id: EntityUri::block(parent_id),
            tags: Vec::new().into(),
            requires: Vec::new(),
            content: content.to_string(),
            content_type: ContentType::Source,
            source_language: Some(lang.parse::<SourceLanguage>().unwrap()),
            source_name: None,
            properties: HashMap::new(),
            marks: None,
            created_at: 0,
            updated_at: 0,
            ..Default::default()
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
            "Test Title\nBody content here",
        );
        block.set_property("ID", Value::String("local://test-uuid".to_string()));

        let file_path = Path::new("/test/file.org");
        let org_text = OrgRenderer::render_entitys(&[block], file_path, &test_doc_uri());

        assert!(org_text.contains("* Test Title"));
        assert!(org_text.contains("Body content here"));
        assert!(org_text.contains(":ID: local://test-uuid"));
    }

    // WP-F: dangling parent must fail loud, not silently drop the block.
    #[test]
    #[should_panic(expected = "projection invariant violated")]
    fn wpf_dangling_parent_panics() {
        let doc = test_doc_uri();
        // Block whose parent is neither the file root nor any block in the set.
        let mut orphan = Block::new_text(
            EntityUri::block("orphan"),
            EntityUri::block("ghost-parent-not-in-set"),
            "Orphan",
        );
        orphan.set_property("ID", Value::String("orphan".to_string()));
        let _ = OrgRenderer::render_entitys(&[orphan], Path::new("/test/file.org"), &doc);
    }

    // WP-F: a parent cycle (present parents, unreachable from the file root)
    // must fail loud rather than silently dropping the whole component.
    #[test]
    #[should_panic(expected = "projection invariant violated")]
    fn wpf_parent_cycle_panics() {
        let doc = test_doc_uri();
        let mut a = Block::new_text(EntityUri::block("a"), EntityUri::block("b"), "A");
        a.set_property("ID", Value::String("a".to_string()));
        let mut b = Block::new_text(EntityUri::block("b"), EntityUri::block("a"), "B");
        b.set_property("ID", Value::String("b".to_string()));
        let _ = OrgRenderer::render_entitys(&[a, b], Path::new("/test/file.org"), &doc);
    }

    // WP-F guard against false positives: a normal tree (roots parented to the
    // file root, children parented to present blocks) renders without panicking.
    #[test]
    fn wpf_normal_tree_does_not_panic() {
        let doc = test_doc_uri();
        let mut root = Block::new_text(EntityUri::block("root"), doc.clone(), "Root");
        root.set_property("ID", Value::String("root".to_string()));
        let mut child =
            Block::new_text(EntityUri::block("child"), EntityUri::block("root"), "Child");
        child.set_property("ID", Value::String("child".to_string()));
        let out = OrgRenderer::render_entitys(&[root, child], Path::new("/test/file.org"), &doc);
        assert!(out.contains("Root"));
        assert!(out.contains("Child"));
    }

    // WP-F: a self-parented row (the filtered-out `sentinel:no_parent` FK anchor
    // shape) must NOT trip the dangling or cycle assertion.
    #[test]
    fn wpf_self_parented_sentinel_does_not_panic() {
        let doc = test_doc_uri();
        let mut root = Block::new_text(EntityUri::block("root"), doc.clone(), "Root");
        root.set_property("ID", Value::String("root".to_string()));
        // Self-parented sentinel-shaped row (id == parent_id) alongside a normal
        // root — the same self-parent shape as the filtered `sentinel:no_parent`.
        let mut sentinel = Block::new_text(
            EntityUri::block("selfanchor"),
            EntityUri::block("selfanchor"),
            "Sentinel",
        );
        sentinel.set_property("ID", Value::String("selfanchor".to_string()));
        let out = OrgRenderer::render_entitys(&[root, sentinel], Path::new("/test/file.org"), &doc);
        assert!(out.contains("Root"));
    }

    #[test]
    fn test_render_entity_with_todo_and_priority() {
        let mut block =
            Block::new_text(EntityUri::block("test-id"), test_doc_uri(), "Task headline");
        block.set_property("ID", Value::String("test-id".to_string()));
        block.set_property("TODO", Value::String("TODO".to_string()));
        block.set_property("PRIORITY", Value::String("A".to_string()));

        let file_path = Path::new("/test/file.org");
        let org_text = OrgRenderer::render_entitys(&[block], file_path, &test_doc_uri());

        assert!(org_text.contains("* TODO [#A] Task headline"));
    }

    #[test]
    fn test_source_blocks_render_before_child_headlines() {
        let doc = test_doc_uri();

        let mut parent =
            Block::new_text(EntityUri::block("parent-id"), doc.clone(), "Parent Heading");
        parent.set_property("ID", Value::String("parent-id".to_string()));

        let mut child_heading = Block::new_text(
            EntityUri::block("child-heading-id"),
            EntityUri::block("parent-id"),
            "Child Heading",
        );
        child_heading.set_property("ID", Value::String("child-heading-id".to_string()));

        let source_block =
            test_source_block("src-id", "parent-id", "holon_prql", "from tasks\n", 1);

        let file_path = Path::new("/test/file.org");
        let blocks = vec![parent, child_heading, source_block];
        let org_text = OrgRenderer::render_entitys(&blocks, file_path, &test_doc_uri());

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

        let mut parent = Block::new_text(EntityUri::block("parent-id"), doc.clone(), "Parent");
        parent.set_property("ID", Value::String("parent-id".to_string()));

        let src1 = test_source_block("src1", "parent-id", "holon_sql", "SELECT 1;\n", 1);
        let src2 = test_source_block("src2", "parent-id", "holon_prql", "from users\n", 2);

        let mut child = Block::new_text(
            EntityUri::block("child-id"),
            EntityUri::block("parent-id"),
            "Child",
        );
        child.set_property("ID", Value::String("child-id".to_string()));

        let file_path = Path::new("/test/file.org");
        let blocks = vec![parent, child, src1, src2];
        let org_text = OrgRenderer::render_entitys(&blocks, file_path, &test_doc_uri());

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

        let mut parent = Block::new_text(EntityUri::block("p"), doc.clone(), "Root");
        parent.set_property("ID", Value::String("p".to_string()));

        let mut text_child =
            Block::new_text(EntityUri::block("t1"), EntityUri::block("p"), "Sub Heading");
        text_child.set_property("ID", Value::String("t1".to_string()));

        let src_child = test_source_block("s1", "p", "python", "print('hi')\n", 10);

        // Deliberately put text_child before src_child in the input vec
        let file_path = Path::new("/test/file.org");
        let blocks = vec![parent, text_child, src_child];
        let org_text = OrgRenderer::render_entitys(&blocks, file_path, &test_doc_uri());

        let src_pos = org_text.find("#+BEGIN_SRC python").expect("source block");
        let sub_pos = org_text.find("** Sub Heading").expect("sub heading");

        assert!(
            src_pos < sub_pos,
            "Source block must come first regardless of input order.\nOutput:\n{}",
            org_text
        );
    }
}
