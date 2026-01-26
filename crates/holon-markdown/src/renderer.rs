//! `Block` tree → Obsidian-flavored markdown.
//!
//! Mirrors `holon_org_format::org_renderer::OrgRenderer`:
//! - `render_document` emits frontmatter (from document properties) +
//!   document preamble + all blocks.
//! - `render_blocks` emits just the block tree, used when the document
//!   row hasn't been loaded yet.
//!
//! Source children render before text children of the same parent (same
//! ordering rule org uses) so the next parse re-attaches them to the same
//! heading rather than to the first nested heading.

use holon_api::block::Block;
use holon_api::types::ContentType;
use holon_api::EntityUri;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::frontmatter::Frontmatter;

pub struct MarkdownRenderer;

impl MarkdownRenderer {
    pub fn render_document(
        doc: &Block,
        blocks: &[Block],
        file_path: &Path,
        file_id: &EntityUri,
    ) -> String {
        let mut out = String::new();
        let fm = frontmatter_from_document(doc);
        out.push_str(&fm.render());

        if !doc.content.is_empty() {
            out.push_str(doc.content.trim_end_matches('\n'));
            out.push('\n');
            // A blank line between preamble and first heading keeps
            // CommonMark parsers happy.
            if !out.ends_with("\n\n") {
                out.push('\n');
            }
        }

        out.push_str(&Self::render_blocks(blocks, file_path, file_id));
        out
    }

    pub fn render_blocks(blocks: &[Block], _file_path: &Path, file_id: &EntityUri) -> String {
        let mut out = String::new();
        let block_map: HashMap<&str, &Block> = blocks.iter().map(|b| (b.id.as_str(), b)).collect();

        let mut roots: Vec<&Block> = blocks.iter().filter(|b| b.parent_id == *file_id).collect();
        roots.sort_by(|a, b| {
            a.sort_key
                .cmp(&b.sort_key)
                .then_with(|| a.id.as_str().cmp(b.id.as_str()))
        });

        for r in roots {
            render_tree(r, &block_map, &mut out, 1);
        }
        out
    }
}

fn render_tree<'a>(
    block: &'a Block,
    map: &HashMap<&'a str, &'a Block>,
    out: &mut String,
    depth: u8,
) {
    match block.content_type {
        ContentType::Text => render_heading(block, depth, out),
        ContentType::Source => render_source(block, out),
        ContentType::Image => render_image(block, out),
    }

    let mut children: Vec<&Block> = map
        .values()
        .copied()
        .filter(|b| b.parent_id == block.id)
        .collect();
    children.sort_by(|a, b| {
        let group = |ct: ContentType| match ct {
            ContentType::Source | ContentType::Image => 0,
            ContentType::Text => 1,
        };
        group(a.content_type)
            .cmp(&group(b.content_type))
            .then_with(|| a.sort_key.cmp(&b.sort_key))
            .then_with(|| a.id.as_str().cmp(b.id.as_str()))
    });

    for c in children {
        let next_depth = if matches!(c.content_type, ContentType::Text) {
            depth.saturating_add(1).min(6)
        } else {
            depth
        };
        render_tree(c, map, out, next_depth);
    }
}

fn render_heading(block: &Block, depth: u8, out: &mut String) {
    let (head, body) = match block.content.split_once('\n') {
        Some((h, b)) => (h, Some(b)),
        None => (block.content.as_str(), None),
    };
    let task_marker = block
        .properties
        .get("task_state")
        .and_then(|v| v.as_string())
        .map(|kw| match kw {
            "DONE" => "[x] ".to_string(),
            "DOING" => "[/] ".to_string(),
            _ => "[ ] ".to_string(),
        })
        .unwrap_or_default();

    let id_marker = block_id_marker(block);

    let hashes = "#".repeat(depth.max(1) as usize);
    out.push_str(&hashes);
    out.push(' ');
    out.push_str(&task_marker);
    out.push_str(head.trim());
    out.push_str(&id_marker);
    out.push('\n');

    if let Some(body) = body {
        let body = body.trim_end_matches('\n');
        if !body.is_empty() {
            out.push('\n');
            out.push_str(body);
            out.push('\n');
        }
    }
    out.push('\n');
}

fn render_source(block: &Block, out: &mut String) {
    let lang = block
        .source_language
        .as_ref()
        .map(|l| format!("{}", l))
        .unwrap_or_default();
    out.push_str("```");
    out.push_str(&lang);
    out.push('\n');
    out.push_str(block.content.trim_end_matches('\n'));
    out.push('\n');
    out.push_str("```\n\n");
}

fn render_image(block: &Block, out: &mut String) {
    // Obsidian embed syntax. `block.content` carries the relative file path.
    out.push_str("![[");
    out.push_str(block.content.trim());
    out.push_str("]]\n\n");
}

fn block_id_marker(block: &Block) -> String {
    // Only emit the trailing `^id` if the ID is a stable Obsidian-style
    // string (alphanumerics + `-`/`_`). UUIDs round-trip too — they match
    // the same charset minus the dashes-are-fine rule.
    let id = block.id.id();
    if id.is_empty() || !id.bytes().all(is_block_id_byte) {
        return String::new();
    }
    format!(" ^{id}")
}

fn is_block_id_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

fn frontmatter_from_document(doc: &Block) -> Frontmatter {
    let title = doc
        .properties
        .get("title")
        .and_then(|v| v.as_string())
        .map(|s| s.to_string());

    // `doc.properties` is the inner HashMap<String,Value> already deserialized
    // from the jsonb properties column. The "tags" key here is a property
    // entry storing a comma-separated string — not the top-level `tags` jsonb
    // column on Block. Field names collide.
    // ALLOW(jsonb_as_string): inner properties value, not the jsonb column.
    let tags: Vec<String> = doc
        .properties
        .get("tags")
        .and_then(|v| v.as_string())
        .map(|csv| {
            csv.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let extra: BTreeMap<String, serde_yaml::Value> = doc
        .properties
        .get("frontmatter_extra")
        .and_then(|v| v.as_string())
        .map(|json| {
            serde_json::from_str::<BTreeMap<String, serde_yaml::Value>>(json).unwrap_or_default()
        })
        .unwrap_or_default();

    Frontmatter { title, tags, extra }
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_api::types::SourceLanguage;
    use std::path::PathBuf;

    fn doc_uri() -> EntityUri {
        EntityUri::file("note.md")
    }

    #[test]
    fn renders_simple_heading() {
        let mut block = Block::new_text(EntityUri::block("h1"), doc_uri(), "Top\nbody text");
        block.set_property("ID", holon_api::Value::String("h1".into()));
        let out =
            MarkdownRenderer::render_blocks(&[block], &PathBuf::from("/test/note.md"), &doc_uri());
        assert!(out.starts_with("# Top ^h1\n"));
        assert!(out.contains("body text"));
    }

    #[test]
    fn nested_heading_uses_higher_depth() {
        let mut top = Block::new_text(EntityUri::block("a"), doc_uri(), "A");
        top.set_property("ID", holon_api::Value::String("a".into()));
        let mut child = Block::new_text(EntityUri::block("b"), EntityUri::block("a"), "B");
        child.set_property("ID", holon_api::Value::String("b".into()));

        let out =
            MarkdownRenderer::render_blocks(&[top, child], &PathBuf::from("/note.md"), &doc_uri());
        assert!(out.contains("# A ^a"));
        assert!(out.contains("## B ^b"));
    }

    #[test]
    fn source_child_renders_before_text_child() {
        let mut parent = Block::new_text(EntityUri::block("p"), doc_uri(), "Parent");
        parent.set_property("ID", holon_api::Value::String("p".into()));

        let mut text = Block::new_text(EntityUri::block("t"), EntityUri::block("p"), "Sub heading");
        text.set_property("ID", holon_api::Value::String("t".into()));

        let mut src = Block::new_source(
            EntityUri::block("s"),
            EntityUri::block("p"),
            "python",
            "print(1)",
        );
        src.set_property("ID", holon_api::Value::String("s".into()));

        let out = MarkdownRenderer::render_blocks(
            &[parent, text, src],
            &PathBuf::from("/note.md"),
            &doc_uri(),
        );
        let src_pos = out.find("```python").expect("source fence present");
        let sub_pos = out.find("## Sub heading").expect("sub heading present");
        assert!(
            src_pos < sub_pos,
            "source must come before nested heading, got:\n{out}"
        );
    }

    #[test]
    fn task_state_renders_task_marker() {
        let mut block = Block::new_text(EntityUri::block("t1"), doc_uri(), "Do thing");
        block.set_property("ID", holon_api::Value::String("t1".into()));
        block.set_property("task_state", holon_api::Value::String("TODO".into()));
        let out = MarkdownRenderer::render_blocks(&[block], &PathBuf::from("/note.md"), &doc_uri());
        assert!(out.contains("# [ ] Do thing ^t1"));
    }

    #[test]
    fn done_task_renders_x_marker() {
        let mut block = Block::new_text(EntityUri::block("t1"), doc_uri(), "Done thing");
        block.set_property("ID", holon_api::Value::String("t1".into()));
        block.set_property("task_state", holon_api::Value::String("DONE".into()));
        let out = MarkdownRenderer::render_blocks(&[block], &PathBuf::from("/note.md"), &doc_uri());
        assert!(out.contains("# [x] Done thing ^t1"));
    }

    #[test]
    fn document_with_frontmatter_renders_yaml_block() {
        let mut doc = Block::new_text(doc_uri(), EntityUri::no_parent(), "note.md");
        doc.set_page(true);
        doc.set_property("title", holon_api::Value::String("My Note".into()));

        let mut head = Block::new_text(EntityUri::block("h"), doc_uri(), "Heading");
        head.set_property("ID", holon_api::Value::String("h".into()));

        let out = MarkdownRenderer::render_document(
            &doc,
            &[head],
            &PathBuf::from("/note.md"),
            &doc_uri(),
        );
        assert!(out.starts_with("---\n"));
        assert!(out.contains("title: My Note"));
        assert!(out.contains("# Heading ^h"));
    }

    #[test]
    fn source_block_renders_with_language() {
        let mut parent = Block::new_text(EntityUri::block("p"), doc_uri(), "P");
        parent.set_property("ID", holon_api::Value::String("p".into()));

        let mut src = Block {
            id: EntityUri::block("s"),
            parent_id: EntityUri::block("p"),
            content: "from x import y".into(),
            content_type: ContentType::Source,
            source_language: Some("holon_prql".parse::<SourceLanguage>().unwrap()),
            ..Block::default()
        };
        src.set_property("ID", holon_api::Value::String("s".into()));

        let out =
            MarkdownRenderer::render_blocks(&[parent, src], &PathBuf::from("/note.md"), &doc_uri());
        assert!(out.contains("```holon_prql\nfrom x import y\n```"));
    }
}
