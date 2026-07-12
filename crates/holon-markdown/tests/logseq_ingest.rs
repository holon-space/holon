//! Directed Tier R/O ingest of a committed LogSeq fixture vault.

use std::path::Path;
use std::path::PathBuf;

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_api::inline_mark::EntityRef;
use holon_api::inline_mark::InlineMark;
use holon_core::file_format::FileFormatAdapter;
use holon_core::file_format::FileFormatParseResult;
use holon_markdown::LogseqMarkdownAdapter;
use holon_markdown::VaultFlavor;
use holon_markdown::build::FOREIGN_OPAQUE_KEY;
use holon_markdown::detect_flavor;
use holon_org_format::OrgBlockExt;

fn vault() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/logseq")
}

fn parse(rel: &str) -> FileFormatParseResult {
    let root = vault();
    let path = root.join(rel);
    let content = std::fs::read_to_string(&path).unwrap();
    LogseqMarkdownAdapter::new()
        .parse(&path, &content, &EntityUri::no_parent(), &root)
        .unwrap()
}

fn find<'a>(blocks: &'a [Block], needle: &str) -> &'a Block {
    blocks
        .iter()
        .find(|b| b.content.contains(needle))
        .unwrap_or_else(|| panic!("no block containing {needle:?}"))
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
fn flavor_detected_from_config() {
    assert_eq!(detect_flavor(&vault()), VaultFlavor::Logseq);
}

#[test]
fn page_document_carries_title_tags_properties() {
    let r = parse("pages/Compat.md");
    assert!(r.document.is_page());
    assert_eq!(r.document.content, "Compat");
    assert!(r.document.tags.contains("compat"));
    assert!(r.document.tags.contains("holon"));
    assert_eq!(
        r.document.get_property_str("type"),
        Some("test page".to_string())
    );
}

#[test]
fn wikilink_and_tags_become_marks_and_tag_field() {
    let r = parse("pages/Compat.md");
    let b = find(&r.blocks, "A block with");
    // [[Page Link]] → Name link mark
    assert!(
        link_targets(b)
            .iter()
            .any(|t| matches!(t, EntityRef::Name { name } if name == "Page Link"))
    );
    // #compat and #[[multi word tag]] → tag edge field
    assert!(b.tags.contains("compat"), "tags: {:?}", b.tags);
    assert!(b.tags.contains("multi word tag"), "tags: {:?}", b.tags);
}

#[test]
fn block_ref_becomes_internal_link_mark() {
    let r = parse("pages/Compat.md");
    let child = find(&r.blocks, "a child referencing");
    assert!(link_targets(child).iter().any(|t| matches!(
        t,
        EntityRef::Internal { id } if id.id() == "11111111-0000-4000-8000-000000000001"
    )));
}

#[test]
fn explicit_id_property_becomes_block_identity() {
    let r = parse("pages/Compat.md");
    let b = find(&r.blocks, "This page exercises");
    assert_eq!(b.id.id(), "11111111-0000-4000-8000-000000000001");
}

#[test]
fn task_markers_and_planning_lift_to_typed_fields() {
    let r = parse("pages/Compat.md");
    let todo = find(&r.blocks, "a task block");
    assert_eq!(
        todo.task_state().map(|t| t.to_string()),
        Some("TODO".to_string())
    );
    assert!(todo.scheduled().is_some(), "scheduled not parsed");

    let done = find(&r.blocks, "finished task");
    assert!(done.task_state().map(|t| t.is_done()).unwrap_or(false));
}

#[test]
fn emphasis_marks_extracted() {
    let r = parse("pages/Compat.md");
    let b = find(&r.blocks, "block.");
    let kinds: Vec<_> = b
        .marks
        .as_ref()
        .unwrap()
        .iter()
        .map(|m| m.mark.loro_key())
        .collect();
    assert!(kinds.contains(&"bold"));
    assert!(kinds.contains(&"italic"));
    assert!(kinds.contains(&"code"));
    // content is stripped of delimiters
    assert!(
        b.content.contains("bold and italic and code"),
        "content: {:?}",
        b.content
    );
}

#[test]
fn tree_hierarchy_via_indentation() {
    let r = parse("pages/Compat.md");
    let parent = find(&r.blocks, "A parent block");
    let child = find(&r.blocks, "a child referencing");
    let grandchild = find(&r.blocks, "a grandchild");
    assert_eq!(child.parent_id, parent.id);
    assert_eq!(grandchild.parent_id, child.id);
    // grandchild property
    assert_eq!(
        grandchild.get_property_str("priority"),
        Some("high".to_string())
    );
}

#[test]
fn unsupported_macro_becomes_disclosed_opaque_block_not_dropped() {
    let r = parse("pages/Compat.md");
    let q = find(&r.blocks, "{{query");
    assert_eq!(
        q.get_property_str(FOREIGN_OPAQUE_KEY),
        Some("query".to_string())
    );
    // Source bytes preserved verbatim.
    assert!(q.content.contains("{{query (todo TODO)}}"));
}

#[test]
fn journal_file_recognized() {
    let r = parse("journals/2026_07_11.md");
    assert!(r.document.tags.contains("journal"), "journal not tagged");
    let entry = find(&r.blocks, "Journal entry");
    assert!(
        link_targets(entry)
            .iter()
            .any(|t| matches!(t, EntityRef::Name { name } if name == "Compat"))
    );
}

#[test]
fn no_block_loss_all_bullets_ingested() {
    let r = parse("pages/Compat.md");
    // 7 top-level bullets + 2 children + 1 grandchild = 10 blocks.
    assert_eq!(
        r.blocks.len(),
        10,
        "block count drifted: {}",
        r.blocks.len()
    );
}
