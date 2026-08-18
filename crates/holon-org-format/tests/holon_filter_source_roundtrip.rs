//! A `holon_filter` source block — the carrier of a bookmarkable filter
//! (FLT-1.b) — is program text, not display content: it must reach the block
//! set as a TYPED source language, and its multi-line body plus its
//! `:polarity` / `:subtree` header args must survive the org round-trip.

use std::path::Path;

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_api::filter::FilterPolarity;
use holon_api::filter::FilterSpec;
use holon_api::filter::SubtreeRule;
use holon_api::types::SourceLanguage;
use holon_org_format::OrgRenderer;
use holon_org_format::parse_org_file;

const FILTER_BODY: &str = "and(eq(\"completed\", 1),\n    not(is_not_null(\"scheduled\")))";

fn parse_filter_page(org: &str) -> Vec<Block> {
    parse_org_file(
        Path::new("/test/Filters.org"),
        org,
        &EntityUri::no_parent(),
        Path::new("/test"),
    )
    .expect("parse Filters.org")
    .blocks
}

/// `holon_filter` is a language the system understands, not an opaque
/// `Other(..)` string. Parse-don't-validate: an unknown language cannot fail
/// loud on a malformed predicate body.
#[test]
fn holon_filter_is_a_typed_source_language() {
    let org = format!(
        "* Hide done tasks\n#+BEGIN_SRC holon_filter :id filters::hide-done::src\n{FILTER_BODY}\n\
         #+END_SRC\n"
    );
    let blocks = parse_filter_page(&org);
    let src = blocks
        .iter()
        .find(|b| b.id.as_str() == "block:filters::hide-done::src")
        .expect("filter source block parsed");

    assert!(
        !matches!(src.source_language, Some(SourceLanguage::Other(_))),
        "holon_filter must reach the block set as a typed language, got {:?}",
        src.source_language
    );
    assert_eq!(src.source_language, Some(SourceLanguage::HolonFilter));
}

/// The multi-line predicate body and the filter's declaration header args are
/// the whole payload — both must survive render → parse unchanged.
#[test]
fn multi_line_body_and_header_args_survive_roundtrip() {
    let page = EntityUri::block("filters");
    let heading = Block::new_text(
        EntityUri::block("filters::hide-done"),
        page.clone(),
        "Hide done tasks",
    );
    let mut filter = Block::new_source(
        EntityUri::block("filters::hide-done::src"),
        EntityUri::block("filters::hide-done"),
        "holon_filter",
        FILTER_BODY,
    );
    filter.set_property("polarity", holon_api::Value::String("hide".into()));
    filter.set_property(
        "subtree",
        holon_api::Value::String("with-descendants".into()),
    );

    let org =
        OrgRenderer::render_entitys(&[heading, filter], Path::new("/test/Filters.org"), &page);
    let blocks = parse_filter_page(&org);
    let src = blocks
        .iter()
        .find(|b| b.id.as_str() == "block:filters::hide-done::src")
        .unwrap_or_else(|| panic!("filter source lost on round-trip; org was:\n{org}"));

    assert_eq!(
        src.content, FILTER_BODY,
        "predicate body must be byte-stable"
    );
    assert_eq!(
        src.get_property_str("polarity").as_deref(),
        Some("hide"),
        "polarity header arg lost; org was:\n{org}"
    );
    assert_eq!(
        src.get_property_str("subtree").as_deref(),
        Some("with-descendants"),
        "subtree header arg lost; org was:\n{org}"
    );
}

/// The end-to-end boundary: org text on disk resolves to a typed
/// [`FilterSpec`] named by its containing headline.
#[test]
fn filter_spec_resolves_from_parsed_org() {
    let org = format!(
        "* Hide done tasks\n\
         #+BEGIN_SRC holon_filter :id filters::hide-done::src :polarity hide :subtree \
         with-descendants\n{FILTER_BODY}\n#+END_SRC\n"
    );
    let blocks = parse_filter_page(&org);
    let head = blocks
        .iter()
        .find(|b| b.content == "Hide done tasks")
        .expect("headline parsed");

    let spec = FilterSpec::parse(&head.id, &blocks).expect("filter resolves");
    assert_eq!(spec.name, "Hide done tasks");
    assert_eq!(spec.polarity, FilterPolarity::Hide);
    assert_eq!(spec.subtree, SubtreeRule::WithDescendants);
    assert!(
        matches!(spec.predicate, holon_api::predicate::Predicate::And(ref p) if p.len() == 2),
        "got {:?}",
        spec.predicate
    );
}

/// A filter's name IS its headline title (FLT-1.b). An empty title is a
/// malformed, unnamed filter — resolution must fail loud, not mint a
/// nameless spec.
#[test]
fn empty_headline_title_is_rejected() {
    let page = EntityUri::block("filters");
    let head = Block::new_text(EntityUri::block("filters::nameless"), page, "");
    let filter = Block::new_source(
        EntityUri::block("filters::nameless::src"),
        EntityUri::block("filters::nameless"),
        "holon_filter",
        "eq(\"completed\", 1)",
    );
    let err =
        FilterSpec::parse(&head.id, &[head.clone(), filter]).expect_err("empty title must fail");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("empty") && msg.contains("name"),
        "expected an empty-title / name error, got: {msg}"
    );
}

/// `:polarity` / `:subtree` are string header args. A non-string Value is a
/// malformed filter — resolution must Err NAMING the offending value, never
/// silently default to Hide/JustBlock (fail-loud, even if unreachable from org
/// today).
#[test]
fn non_string_polarity_is_rejected() {
    let head = Block::new_text(
        EntityUri::block("filters::hd"),
        EntityUri::block("filters"),
        "Hide done tasks",
    );
    let mut filter = Block::new_source(
        EntityUri::block("filters::hd::src"),
        EntityUri::block("filters::hd"),
        "holon_filter",
        "eq(\"completed\", 1)",
    );
    filter.set_property("polarity", holon_api::Value::Integer(7));
    let err = FilterSpec::parse(&head.id, &[head.clone(), filter])
        .expect_err("non-string polarity must fail");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("polarity must be a string"),
        "expected a non-string polarity error, got: {msg}"
    );
    assert!(
        msg.contains("Integer") && msg.contains('7'),
        "error must name the offending value, got: {msg}"
    );
}
