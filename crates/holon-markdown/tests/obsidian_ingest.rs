//! Directed Tier R/O ingest of a committed Obsidian fixture vault.

use std::path::Path;
use std::path::PathBuf;

use holon_api::block::Block;
use holon_api::inline_mark::EntityRef;
use holon_api::inline_mark::InlineMark;
use holon_api::EntityUri;
use holon_core::file_format::FileFormatAdapter;
use holon_core::file_format::FileFormatParseResult;
use holon_markdown::build::FOREIGN_OPAQUE_KEY;
use holon_markdown::detect_flavor;
use holon_markdown::ObsidianMarkdownAdapter;
use holon_markdown::VaultFlavor;
use holon_org_format::OrgBlockExt;

fn vault() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/obsidian")
}

fn parse(rel: &str) -> FileFormatParseResult {
    let root = vault();
    let path = root.join(rel);
    let content = std::fs::read_to_string(&path).unwrap();
    ObsidianMarkdownAdapter::new()
        .parse(&path, &content, &EntityUri::no_parent(), &root)
        .unwrap()
}

fn find<'a>(blocks: &'a [Block], needle: &str) -> &'a Block {
    blocks
        .iter()
        .find(|b| b.content.contains(needle))
        .unwrap_or_else(|| panic!("no block containing {needle:?}"))
}

fn opaque_of<'a>(blocks: &'a [Block], kind: &str) -> &'a Block {
    blocks
        .iter()
        .find(|b| b.get_property_str(FOREIGN_OPAQUE_KEY).as_deref() == Some(kind))
        .unwrap_or_else(|| panic!("no opaque block of kind {kind:?}"))
}

fn link_targets(b: &Block) -> Vec<EntityRef> {
    b.marks
        .as_ref()
        .map(|ms| {
            ms.iter()
                .filter_map(|m| match &m.mark {
                    InlineMark::Link { target, .. } => Some(target.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn flavor_detected_from_dot_obsidian() {
    assert_eq!(detect_flavor(&vault()), VaultFlavor::Obsidian);
}

#[test]
fn frontmatter_becomes_document_properties_tags_aliases() {
    let r = parse("Compat.md");
    assert!(r.document.is_page());
    assert_eq!(r.document.content, "Compat");
    assert!(r.document.tags.contains("compat"));
    assert!(r.document.tags.contains("holon/test"));
    // aliases hoisted to tags (alternate names) in the R/O spike
    assert!(r.document.tags.contains("Compat Playground"));
    assert_eq!(r.document.get_property_str("rating"), Some("5".to_string()));
}

#[test]
fn headings_form_the_block_tree() {
    let r = parse("Compat.md");
    let h = find(&r.blocks, "Links and references");
    let para = find(&r.blocks, "This note links");
    assert_eq!(para.parent_id, h.id);
}

#[test]
fn wikilinks_and_alias_and_inline_tag_in_paragraph() {
    let r = parse("Compat.md");
    let para = find(&r.blocks, "This note links");
    let ts = link_targets(para);
    assert!(ts
        .iter()
        .any(|t| matches!(t, EntityRef::Name { name } if name == "Sample Project")));
    assert!(ts
        .iter()
        .any(|t| matches!(t, EntityRef::Name { name } if name == "Dangling Target")));
    assert!(
        para.tags.contains("compat"),
        "inline #compat not extracted: {:?}",
        para.tags
    );
    // alias label preserved
    assert!(
        para.content.contains("the project"),
        "alias label lost: {:?}",
        para.content
    );
}

#[test]
fn checkbox_list_items_become_tasks() {
    let r = parse("Compat.md");
    let open = find(&r.blocks, "open task");
    assert_eq!(
        open.task_state().map(|t| t.to_string()),
        Some("TODO".to_string())
    );
    let done = find(&r.blocks, "done task");
    assert!(done.task_state().map(|t| t.is_done()).unwrap_or(false));
}

#[test]
fn trailing_block_anchor_becomes_block_id() {
    let r = parse("Compat.md");
    let b = find(&r.blocks, "carries a block ID");
    assert_eq!(b.id.id(), "anchor1");
    assert!(
        !b.content.contains('^'),
        "anchor not stripped: {:?}",
        b.content
    );
}

#[test]
fn callout_comment_code_become_disclosed_opaque_blocks() {
    let r = parse("Compat.md");
    let callout = opaque_of(&r.blocks, "callout");
    assert!(callout.content.contains("[!note]"));
    let comment = opaque_of(&r.blocks, "comment");
    assert!(comment.content.contains("hidden comment"));
    let code = opaque_of(&r.blocks, "code");
    assert!(code.content.contains("fn main"));
}

#[test]
fn subfolder_note_ingests_with_anchor() {
    let r = parse("Projects/Sample Project.md");
    assert_eq!(r.document.content, "Sample Project");
    assert!(r.document.tags.contains("project"));
    let goal = find(&r.blocks, "round-trip fidelity");
    assert_eq!(goal.id.id(), "goalblock");
}
