//! Characterization of the org render → parse round trip on the input shapes
//! the existing generators exclude: content that COLLIDES WITH ORG INLINE
//! MARKUP (`__x__`, `*x*`, `~x~`, …), EMPTY content, and EMPTY titles.
//!
//! `holon_block_roundtrip_testing::valid_title` is `[a-zA-Z0-9][a-zA-Z0-9
//! ]{0,48}[a-zA-Z0-9]` and `valid_body` is `[a-zA-Z0-9 .,!?\n]{10,200}` — both
//! forbid every markup character and both forbid the empty string, so
//! `round_trip_pbt.rs` structurally cannot see any of this.
//!
//! Property under test: a block's `content` survives
//! `render_document → parse_org_file` unchanged.

use std::path::Path;

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_org_format::OrgRenderer;
use holon_org_format::parse_org_file;

const ROOT: &str = "/test";
const FILE: &str = "/test/page.org";

fn doc_root() -> Block {
    let mut doc = Block::new_text(EntityUri::block("page-root"), EntityUri::no_parent(), "");
    doc.set_page(true);
    doc
}

/// Render `blocks` under a `#+ID: page-root` doc and parse the result back.
/// Returns `(rendered_org, parsed_blocks)`.
fn roundtrip(blocks: &[Block]) -> (String, Vec<Block>) {
    let doc = doc_root();
    let org = OrgRenderer::render_document(&doc, blocks, Path::new(FILE), &doc.id);
    let parsed = parse_org_file(
        Path::new(FILE),
        &org,
        &EntityUri::no_parent(),
        Path::new(ROOT),
    )
    .expect("parse the org this renderer just produced");
    (org, parsed.blocks)
}

/// Round-trip a single headline whose content is `content`; return what came
/// back as that block's content.
fn content_after_roundtrip(content: &str) -> String {
    let block = Block::new_text(
        EntityUri::block("b1"),
        EntityUri::block("page-root"),
        content,
    );
    let (org, parsed) = roundtrip(&[block]);
    parsed
        .iter()
        .find(|b| b.id.as_str() == "block:b1")
        .unwrap_or_else(|| {
            panic!(
                "block:b1 vanished from the round trip.\n--- rendered org ---\n{org}\n--- parsed \
                 ids ---\n{:?}",
                parsed.iter().map(|b| b.id.as_str()).collect::<Vec<_>>()
            )
        })
        .content
        .clone()
}

fn assert_content_survives(content: &str) {
    let got = content_after_roundtrip(content);
    assert_eq!(
        content, got,
        "block content must survive the org round trip"
    );
}

// ===========================================================================
// Family 1 — content that collides with org inline markup
// ===========================================================================

/// Task #67, CONFIRMED in prod: `block:__default__`'s content `__default__`
/// comes back as `default`. `_x_` is org UNDERLINE, so `__default__` parses as
/// underline-inside-underline and both delimiter pairs are consumed into marks.
#[test]
fn dunder_content_survives_roundtrip() {
    assert_content_survives("__default__");
}

#[test]
fn single_underscore_content_survives_roundtrip() {
    assert_content_survives("_default_");
}

#[test]
fn star_content_survives_roundtrip() {
    assert_content_survives("*bold-looking*");
}

#[test]
fn slash_content_survives_roundtrip() {
    assert_content_survives("/italic-looking/");
}

#[test]
fn tilde_content_survives_roundtrip() {
    assert_content_survives("~tilde-looking~");
}

#[test]
fn equals_content_survives_roundtrip() {
    assert_content_survives("=verbatim-looking=");
}

#[test]
fn plus_content_survives_roundtrip() {
    assert_content_survives("+strike-looking+");
}

/// Markup-looking text mixed with ordinary words — the realistic dogfood shape
/// (an identifier inside a sentence).
#[test]
fn dunder_identifier_inside_a_sentence_survives_roundtrip() {
    assert_content_survives("the __default__ profile is used");
}

/// Bracket text that is not a well-formed link.
#[test]
fn bare_double_bracket_content_survives_roundtrip() {
    assert_content_survives("see [[not-a-link");
}

/// A block whose content carries a LINK must re-render its link markup
/// byte-identically. Unlike [`assert_content_survives`], which compares the
/// STRIPPED content (a link's content is just its label), this asserts on the
/// emitted org bytes — which is where a mangled link target shows up.
fn assert_org_bytes_survive(org: &str) {
    let (content, marks) = holon_org_format::extract_inline_marks(org);
    let mut block = Block::new_text(
        EntityUri::block("link-bytes"),
        EntityUri::block("parent"),
        &content,
    );
    block.marks = Some(marks);
    let (emitted, _) = holon_org_format::render_block_content_checked(&block);
    assert_eq!(org, emitted, "link markup must survive extract → render");
}

/// A colon in a LATER path segment does not make a target scheme-shaped: the
/// scheme shape is a property of the WHOLE target, so `Meeting/Notes:2026` is
/// an ordinary wiki page path that happens to contain a colon.
///
/// `LinkTarget::UnknownScheme` covers BOTH shapes (`link_parser.rs:172` for the
/// whole target, `:196` for a scheme-shaped segment), so anything that turns
/// that one verdict into an `EntityUri` mints `block:Meeting/Notes:2026` and
/// REWRITES the user's text. Link text is authored data — no round trip may
/// ever edit it.
#[test]
fn colon_in_a_later_path_segment_survives_roundtrip() {
    assert_org_bytes_survive("see [[Meeting/Notes:2026]] here");
    assert_org_bytes_survive("see [[Projects/Q3:plan][the plan]] here");
    assert_org_bytes_survive("see [[Areas/cc-session:abc]] here");
}

/// The other half of the same rule: a WHOLE scheme-shaped target is a genuine
/// entity link and must ALSO come back byte-identically, registered or not.
#[test]
fn whole_scheme_shaped_target_survives_roundtrip() {
    assert_org_bytes_survive("see [[t-widget:abc123]] here");
    assert_org_bytes_survive("see [[t-widget:abc123][Widget]] here");
    assert_org_bytes_survive("see [[Areas:Work]] here");
    assert_org_bytes_survive("see [[block:abc-123][a block]] here");
}

/// A DOUBLE colon after the scheme (`tag::x`) is a well-formed URI whose path
/// simply starts with `:`. It is scheme-shaped, its scheme is registered, and
/// `EntityUri::parse` accepts it — so nothing about it is malformed and the
/// authored bytes must come back untouched.
///
/// The trap is `EntityUri::from_raw`, whose colon-leading-path guard exists to
/// keep BARE synthetic ids (`root-layout::src::0`) off the entity path and
/// excepts only `block`. Reached from authored link text it rejects `tag::x`
/// and re-mints it as `block:tag::x`, so the round trip emits
/// `[[block:tag::x][tag::x]]` — the author's link silently rewritten.
#[test]
fn double_colon_scheme_target_survives_roundtrip() {
    assert_org_bytes_survive("see [[tag::x]] here");
    assert_org_bytes_survive("see [[person::odd]] here");
    assert_org_bytes_survive("see [[tag::x][the tag]] here");
    assert_org_bytes_survive("see [[person::odd][that person]] here");
}

/// The same arm's other input: a REGISTERED scheme whose path is not a legal
/// URI (`tag:a b` — a space is not URI-encodable). It is still authored text,
/// so it too must come back verbatim rather than through a re-mint that
/// cannot even build a URI to hold it.
#[test]
fn unparseable_registered_scheme_target_survives_roundtrip() {
    assert_org_bytes_survive("see [[tag:a b]] here");
    assert_org_bytes_survive("see [[tag:a b][spaced]] here");
}

// ===========================================================================
// Family 2 — empty content
// ===========================================================================

/// `inv-blocks-match-ref/org`: blocks whose content is `""`. Renders as `"* "`.
#[test]
fn empty_content_block_survives_roundtrip() {
    assert_content_survives("");
}

/// An empty-content block that OWNS CHILDREN — the structural-page shape.
#[test]
fn empty_content_block_with_children_survives_roundtrip() {
    let parent = Block::new_text(EntityUri::block("p"), EntityUri::block("page-root"), "");
    let child = Block::new_text(EntityUri::block("c"), EntityUri::block("p"), "child");
    let (org, parsed) = roundtrip(&[parent, child]);
    let ids: Vec<&str> = parsed.iter().map(|b| b.id.as_str()).collect();
    assert!(
        ids.contains(&"block:p") && ids.contains(&"block:c"),
        "both blocks must survive.\n--- org ---\n{org}\n--- ids ---\n{ids:?}"
    );
    let c = parsed.iter().find(|b| b.id.as_str() == "block:c").unwrap();
    assert_eq!(
        c.parent_id.as_str(),
        "block:p",
        "child must stay parented to the empty-content block.\n--- org ---\n{org}"
    );
}

/// Several empty-content SIBLINGS — the ~17-empty-heading disk shape.
#[test]
fn empty_content_siblings_all_survive_roundtrip() {
    let blocks: Vec<Block> = (0..5)
        .map(|i| {
            Block::new_text(
                EntityUri::block(&format!("e{i}")),
                EntityUri::block("page-root"),
                "",
            )
        })
        .collect();
    let (org, parsed) = roundtrip(&blocks);
    let ids: Vec<&str> = parsed.iter().map(|b| b.id.as_str()).collect();
    for i in 0..5 {
        let want = format!("block:e{i}");
        assert!(
            ids.contains(&want.as_str()),
            "empty-content sibling {want} must survive.\n--- org ---\n{org}\n--- ids ---\n{ids:?}"
        );
    }
}

/// An empty-content block whose BODY is non-empty (`"\nbody text"`): title
/// empty, body present.
#[test]
fn empty_title_with_body_survives_roundtrip() {
    assert_content_survives("\nbody text");
}

// ===========================================================================
// Family 3 — fixed point (the echo-loop invariant, at format level)
// ===========================================================================

/// `inv-org-render-fixed-point` at the format layer: re-rendering what the
/// parser gave back must reproduce the same bytes. A second pass that differs
/// is an echo-loop — `re_render_all_tracked` would rewrite the file forever.
fn assert_render_is_fixed_point(blocks: &[Block]) {
    let (org1, parsed1) = roundtrip(blocks);
    let doc = doc_root();
    let org2 = OrgRenderer::render_document(&doc, &parsed1, Path::new(FILE), &doc.id);
    assert_eq!(
        org1, org2,
        "render(parse(render(x))) must equal render(x) — otherwise every \
         write-back rewrites the file"
    );
}

#[test]
fn dunder_content_is_render_fixed_point() {
    assert_render_is_fixed_point(&[Block::new_text(
        EntityUri::block("b1"),
        EntityUri::block("page-root"),
        "__default__",
    )]);
}

#[test]
fn empty_content_siblings_are_render_fixed_point() {
    let blocks: Vec<Block> = (0..5)
        .map(|i| {
            Block::new_text(
                EntityUri::block(&format!("e{i}")),
                EntityUri::block("page-root"),
                "",
            )
        })
        .collect();
    assert_render_is_fixed_point(&blocks);
}

/// The `org-render-echo-loop` disk/render delta is exactly one headline whose
/// RENDER carries `[[block:<uuid>][c1]]` while DISK carries bare `c1`. If a
/// link-bearing headline title does not round-trip through parse+render, that
/// asymmetry alone sustains the echo.
#[test]
fn headline_title_with_internal_link_is_render_fixed_point() {
    let org1 = format!(
        "#+ID: page-root\n* [[block:378e8264-6d30-8df8-795d-c57851849f74][c1]]\n:PROPERTIES:\n:ID: \
         c1\n:END:\n"
    );
    let parsed = parse_org_file(
        Path::new(FILE),
        &org1,
        &EntityUri::no_parent(),
        Path::new(ROOT),
    )
    .expect("parse");
    let org2 = OrgRenderer::render_document(
        &parsed.document,
        &parsed.blocks,
        Path::new(FILE),
        &parsed.document.id,
    );
    assert_eq!(
        org1, org2,
        "a link-bearing headline title must re-render to the same bytes"
    );
}

// ===========================================================================
// Family 4 — task #59 residuals: doc-root preamble handling
// ===========================================================================

/// The doc-root's own body (the pre-first-headline preamble) must survive.
/// `render_document` emits `content.split_once('\n').1.trim_matches('\n')`;
/// the parser applies `str::trim` to the whole preamble.
fn preamble_after_roundtrip(preamble: &str) -> String {
    let mut doc = doc_root();
    doc.content = format!("\n{preamble}");
    let child = Block::new_text(
        EntityUri::block("b1"),
        EntityUri::block("page-root"),
        "heading",
    );
    let org = OrgRenderer::render_document(&doc, &[child], Path::new(FILE), &doc.id);
    let parsed = parse_org_file(
        Path::new(FILE),
        &org,
        &EntityUri::no_parent(),
        Path::new(ROOT),
    )
    .expect("parse");
    parsed
        .document
        .content
        .split_once('\n')
        .map(|(_, rest)| rest.to_string())
        .unwrap_or_default()
}

#[test]
fn indented_first_preamble_line_survives_roundtrip() {
    let preamble = "    indented first line\n    indented second line";
    assert_eq!(preamble, preamble_after_roundtrip(preamble));
}

#[test]
fn plain_preamble_survives_roundtrip() {
    let preamble = "just a preamble line";
    assert_eq!(preamble, preamble_after_roundtrip(preamble));
}

/// The doc-root preamble is the OTHER verbatim-emit site. It must not lose
/// markup-shaped literals either.
#[test]
fn markup_shaped_preamble_survives_roundtrip() {
    let preamble = "the __default__ profile and *stars*";
    assert_eq!(preamble, preamble_after_roundtrip(preamble));
}

// ===========================================================================
// Family 5 — body text (the second half of a block's content)
// ===========================================================================

/// A block's `content` is `title\nbody` and the parser extracts marks from the
/// WHOLE thing, so the body half is exposed to exactly the same loss.
#[test]
fn markup_shaped_body_survives_roundtrip() {
    assert_content_survives("plain heading\nthe __default__ profile is used");
}
