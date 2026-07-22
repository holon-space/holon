//! Regression: a source block that renders as a TOP-LEVEL `#+BEGIN_SRC` under
//! the file `#+ID:` header (no enclosing `* headline`) must survive the org
//! round-trip. This is the shape `convert_block_to_page` produces when the
//! converted block owns a source child (row-28 keystone RED: the journals
//! `holon_rule` action moved into its own page file and was silently dropped by
//! the parser, which only walked `doc.headlines()`).

use std::path::Path;

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_org_format::OrgRenderer;
use holon_org_format::parse_org_file;

/// A page whose direct child is a `holon_rule` source block — the exact
/// on-disk content `convert_block_to_page(journals::auto-create)` writes.
#[test]
fn top_level_source_under_page_id_survives_parse() {
    let content = "#+ID: 7b512cac-d64f-e768-7a50-ac1e5a8f25f8\n\
#+BEGIN_SRC holon_rule :id journals::action::0\n\
name: daily_journal\n\
when: 'not block_exists(\"Journals/{today}\")'\n\
emit:\n  place: page(journals)\n  name: \"{today}\"\n\
#+END_SRC\n";

    let parsed = parse_org_file(
        Path::new("/test/Journals/Journal Auto-Create.org"),
        content,
        &EntityUri::no_parent(),
        Path::new("/test"),
    )
    .expect("parse");
    let rule = parsed
        .blocks
        .iter()
        .find(|b| b.id.as_str() == "block:journals::action::0")
        .expect("top-level holon_rule source must be parsed as a block");
    assert_eq!(rule.content_type, holon_api::ContentType::Source);
    assert_eq!(
        rule.parent_id, parsed.document.id,
        "top-level source parents to the doc root"
    );
}

/// Full render -> parse fixed point for a page carrying BOTH top-level display
/// sources (query + render) and a headline that owns a source child. Proves the
/// top-level pass does not disturb the existing headline-source path.
#[test]
fn full_journals_layout_render_parse_roundtrip() {
    let file_id = EntityUri::block("journals");
    let src = Block::new_source(
        EntityUri::block("journals::src::0"),
        file_id.clone(),
        "holon_sql",
        "SELECT * FROM journal_feed ORDER BY content DESC",
    );
    let render = Block::new_source(
        EntityUri::block("journals::render::0"),
        file_id.clone(),
        "render",
        "list(#{})",
    );
    let heading = Block::new_text(
        EntityUri::block("journals::auto-create"),
        file_id.clone(),
        "Journal Auto-Create",
    );
    let rule = Block::new_source(
        EntityUri::block("journals::action::0"),
        EntityUri::block("journals::auto-create"),
        "holon_rule",
        "name: daily_journal",
    );

    let org = OrgRenderer::render_entitys(
        &[src, render, heading, rule],
        Path::new("/test/Journals.org"),
        &file_id,
    );
    let parsed = parse_org_file(
        Path::new("/test/Journals.org"),
        &org,
        &EntityUri::no_parent(),
        Path::new("/test"),
    )
    .expect("parse");
    let ids: Vec<&str> = parsed.blocks.iter().map(|b| b.id.as_str()).collect();
    for want in [
        "block:journals::src::0",
        "block:journals::render::0",
        "block:journals::auto-create",
        "block:journals::action::0",
    ] {
        assert!(
            ids.contains(&want),
            "{want} lost on round-trip; got {ids:?}"
        );
    }
}
