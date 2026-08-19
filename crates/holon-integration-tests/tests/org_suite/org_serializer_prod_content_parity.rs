//! @pbt kind harness
//! @pbt covers org-serializer-prod-content-parity — the harness org serializer
//! must emit a block's content byte-identically to what prod's write-back puts
//! on disk, so harness expectations are prod-faithful by construction.
//!
//! The oracle is the REAL write-back renderer, `OrgRenderer::render_entitys`
//! (`crates/holon-org-format/src/org_renderer.rs`) — what
//! `WritebackRenderer::render_blocks`
//! (`crates/holon-filesystem/src/writeback_render.rs`) reaches on every file
//! write. Asserting against the inner `render_block_content` instead would be
//! near-tautological: that is the very function the harness delegates to, so
//! the test would stay green even if prod's chain moved off it.

use holon_api::EntityUri;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::block::Block;
use holon_integration_tests::serialize_blocks_to_org_with_doc;
use holon_orgmode::OrgRenderer;
use holon_orgmode::models::OrgBlockExt;

/// `__default__` is markup-shaped: org reads `__…__` as emphasis, so an
/// un-quoted emission parses back as `default`.
const MARKUP_SHAPED: &str = "__default__";

/// A renderer's bytes for a single level-1 block with the headline stars and
/// the `:PROPERTIES:` drawer removed — the content the two renderers must
/// agree on, and none of the scaffolding they are entitled to derive
/// differently.
fn content_lines(org: &str) -> Vec<String> {
    let mut lines = org.lines();
    let headline = lines
        .next()
        .expect("renderer emitted nothing")
        .strip_prefix("* ")
        .expect("first line is not a level-1 headline")
        .to_string();

    let mut out = vec![headline];
    let mut in_drawer = false;
    for line in lines {
        match line {
            ":PROPERTIES:" => in_drawer = true,
            ":END:" => in_drawer = false,
            _ if in_drawer || line.is_empty() => {}
            _ => out.push(line.to_string()),
        }
    }
    out
}

fn assert_parity(block: &Block, doc: &EntityUri) {
    let prod = OrgRenderer::render_entitys(
        std::slice::from_ref(block),
        std::path::Path::new("/vault/page.org"),
        doc,
    );
    let harness = serialize_blocks_to_org_with_doc(&[block], doc, None);

    assert_eq!(
        content_lines(&harness),
        content_lines(&prod),
        "harness serializer diverges from prod's write-back render\nharness:\n{harness}\nprod:\n\
         {prod}"
    );
}

#[test]
fn markup_shaped_literal_matches_prod_writeback() {
    let doc = EntityUri::block("doc_id");
    let block = Block::new_text(EntityUri::block("abc"), doc.clone(), MARKUP_SHAPED);

    assert_parity(&block, &doc);
}

/// Crossing marks are the divergence in the OPPOSITE direction: org cannot
/// nest them, so a renderer that emits the delimiters naively INJECTS markup
/// into the content (`abcdefgh` came back out as `*abc/de*fgh/`). Prod's ladder
/// degrades instead — the harness must degrade identically.
#[test]
fn crossing_marks_match_prod_writeback() {
    let doc = EntityUri::block("doc_id");
    let mut block = Block::new_text(EntityUri::block("abc"), doc.clone(), "abcdefgh");
    block.marks = Some(vec![
        MarkSpan::new(0, 6, InlineMark::Bold),
        MarkSpan::new(4, 8, InlineMark::Italic),
    ]);

    assert_parity(&block, &doc);
}

#[test]
fn nested_marks_match_prod_writeback() {
    let doc = EntityUri::block("doc_id");
    let mut block = Block::new_text(EntityUri::block("abc"), doc.clone(), "abcdefgh");
    block.marks = Some(vec![
        MarkSpan::new(0, 8, InlineMark::Bold),
        MarkSpan::new(2, 5, InlineMark::Italic),
    ]);

    assert_parity(&block, &doc);
}

#[test]
fn multiline_body_matches_prod_writeback() {
    let doc = EntityUri::block("doc_id");
    let block = Block::new_text(
        EntityUri::block("abc"),
        doc.clone(),
        "__title__\nbody with =literal= and __more__",
    );

    assert_parity(&block, &doc);
}

#[test]
fn harness_serialization_round_trips_markup_shaped_content() {
    let doc = EntityUri::block("doc_id");
    let block = Block::new_text(EntityUri::block("abc"), doc.clone(), MARKUP_SHAPED);

    let out = serialize_blocks_to_org_with_doc(&[&block], &doc, None);
    let parsed = holon_orgmode::parse_org_file(
        std::path::Path::new("/vault/page.org"),
        &out,
        &EntityUri::no_parent(),
        std::path::Path::new("/vault"),
    )
    .expect("harness output must parse");

    let reparsed = parsed
        .blocks
        .iter()
        .find(|b| b.id.id() == "abc")
        .expect("block abc lost on re-parse");

    assert_eq!(
        reparsed.content, MARKUP_SHAPED,
        "harness serialization corrupts markup-shaped content\nfile:\n{out}"
    );
}

/// The stars are the ONE piece of scaffolding the two renderers derive
/// differently (prod from `block.level()`, the harness from recursion depth).
/// Pinning the default keeps `content_lines`' `"* "` strip honest instead of
/// silently comparing mis-aligned lines.
#[test]
fn both_renderers_emit_level_one_for_a_root_block() {
    let block = Block::new_text(
        EntityUri::block("abc"),
        EntityUri::block("doc_id"),
        "plain title",
    );
    assert_eq!(block.level(), 1);
}
