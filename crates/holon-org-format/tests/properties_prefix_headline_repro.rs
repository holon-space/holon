//! Pin for the `:PROPERTIES:`-prefix headline finding (extended-gen PBT,
//! Phase 3 axis 1).
//!
//! Verdict: BY-DESIGN org semantics, not a renderer/parser bug. A headline
//! whose text ends with a `:word:` group is org TAG syntax — org has no
//! escape for it — so on re-parse the group moves from `content` into
//! `block.tags` and the system converges to that fixed point. The PBT
//! reference model mirrors this via `split_headline_tags` (same grammar the
//! parser runs). These tests pin both the split and the fixed point.

use holon_api::block::Block;
use holon_api::{EntityUri, Value};
use holon_org_format::parser::split_headline_tags;
use holon_org_format::{parse_org_file, OrgRenderer};
use std::path::Path;

fn make_block(id: &str, parent: &EntityUri, content: &str) -> Block {
    let uri = EntityUri::block(id);
    let mut b = Block::new_text(uri, parent.clone(), content);
    b.set_property("ID", Value::String(id.to_string()));
    b
}

fn render_parse(blocks: &[Block]) -> Vec<Block> {
    let file_id = EntityUri::from_raw("file:test.org");
    let doc = Block::new_text(file_id.clone(), EntityUri::no_parent(), "test.org");
    let rendered = OrgRenderer::render_document(&doc, blocks, Path::new("test.org"), &file_id);
    eprintln!("--- rendered ---\n{rendered}\n----------------");
    parse_org_file(
        Path::new("test.org"),
        &rendered,
        &EntityUri::no_parent(),
        Path::new("."),
    )
    .unwrap()
    .blocks
}

#[test]
fn split_headline_tags_mirrors_parser() {
    assert_eq!(
        split_headline_tags(":PROPERTIES:"),
        (String::new(), vec!["PROPERTIES".to_string()])
    );
    assert_eq!(
        split_headline_tags(":PROPERTIES: 25xyvg S"),
        (":PROPERTIES: 25xyvg S".to_string(), vec![])
    );
    assert_eq!(
        split_headline_tags("a :t1:t2:"),
        ("a".to_string(), vec!["t1".to_string(), "t2".to_string()])
    );
    // Not tags per org grammar: empty group, inner space, no leading whitespace.
    assert_eq!(split_headline_tags("a ::"), ("a ::".to_string(), vec![]));
    assert_eq!(
        split_headline_tags("a :a :"),
        ("a :a :".to_string(), vec![])
    );
    assert_eq!(split_headline_tags("a:b:"), ("a:b:".to_string(), vec![]));
    // No TODO-keyword stripping in this seam.
    assert_eq!(
        split_headline_tags("TODO x"),
        ("TODO x".to_string(), vec![])
    );
    assert_eq!(split_headline_tags(""), (String::new(), vec![]));
}

/// `:PROPERTIES:`-PREFIX (more text after) round-trips verbatim — only a
/// trailing tag group is special.
#[test]
fn properties_prefix_headline_round_trips() {
    let file_id = EntityUri::from_raw("file:test.org");
    let parsed = render_parse(&[
        make_block("4ar2ch--y0", &file_id, ":PROPERTIES: 25xyvg S"),
        make_block("m85m9", &file_id, "B4 o"),
    ]);

    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].id.as_str(), "block:4ar2ch--y0");
    assert_eq!(parsed[0].content, ":PROPERTIES: 25xyvg S");
    assert_eq!(parsed[1].id.as_str(), "block:m85m9");
    assert_eq!(parsed[1].content, "B4 o");
}

/// A headline whose text IS a tag group (`:PROPERTIES:`) parses to empty
/// content + the tag, keeps its `:ID:`, and is a render→parse fixed point.
#[test]
fn trailing_tag_group_moves_to_tags_and_is_fixed_point() {
    use holon_org_format::models::OrgBlockExt;

    let file_id = EntityUri::from_raw("file:test.org");
    let parsed = render_parse(&[
        make_block("4ar2ch--y0", &file_id, ":PROPERTIES:"),
        make_block("split-new", &file_id, " 25xyvg S"),
    ]);

    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].id.as_str(), "block:4ar2ch--y0");
    assert_eq!(parsed[0].content, "");
    assert_eq!(parsed[0].tags().to_vec(), vec!["PROPERTIES".to_string()]);
    assert_eq!(parsed[1].id.as_str(), "block:split-new");
    assert_eq!(parsed[1].content, "25xyvg S");

    // Second round trip is the identity — the system converges, nothing is
    // silently dropped.
    let parsed2 = render_parse(&parsed);
    assert_eq!(parsed2.len(), 2);
    assert_eq!(parsed2[0].content, "");
    assert_eq!(parsed2[0].tags().to_vec(), vec!["PROPERTIES".to_string()]);
    assert_eq!(parsed2[1].content, "25xyvg S");
}

/// Body (non-first) lines spelling drawer syntax survive verbatim.
#[test]
fn properties_in_body_round_trips() {
    let file_id = EntityUri::from_raw("file:test.org");
    let parsed = render_parse(&[
        make_block("h1", &file_id, "title\n:PROPERTIES: x\nmore"),
        make_block("h2", &file_id, "t2\n:PROPERTIES:\n:FOO: bar\n:END:\ntail"),
    ]);

    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].content, "title\n:PROPERTIES: x\nmore");
    assert_eq!(
        parsed[1].content,
        "t2\n:PROPERTIES:\n:FOO: bar\n:END:\ntail"
    );
}
