//! Probe: values the org renderer corrupts or drops on a render -> parse
//! round trip. Each test prints what it sees and asserts the faithful
//! outcome, so a red run documents the hazard.

use std::path::Path;

use holon_api::EntityUri;
use holon_api::Tags;
use holon_api::Value;
use holon_api::block::Block;
use holon_org_format::OrgBlockExt;
use holon_org_format::OrgRenderer;
use holon_org_format::parse_org_file;

const FILE: &str = "/vault/p.org";

fn parse(source: &str) -> (Block, Vec<Block>) {
    let parsed = parse_org_file(
        Path::new(FILE),
        source,
        &EntityUri::no_parent(),
        Path::new("/vault"),
    )
    .expect("parse");
    (parsed.document, parsed.blocks)
}

fn round_trip(kid: Block) -> (String, Option<Block>) {
    let (document, mut blocks) = parse("#+ID: p\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n");
    let id = kid.id.clone();
    blocks.push(kid);
    let text = OrgRenderer::render_document(&document, &blocks, Path::new(FILE), &document.id);
    let (_, reparsed) = parse(&text);
    let back = reparsed.into_iter().find(|b| b.id == id);
    (text, back)
}

fn kid(content: &str) -> Block {
    Block::new_text(
        EntityUri::block("kid"),
        EntityUri::block("topic"),
        content.to_string(),
    )
}

#[test]
fn hazard1_multi_line_text_property_reaches_the_drawer_raw() {
    let mut b = kid("Kid");
    b.set_property(
        "note",
        Value::String("line one\n* Evil heading\n:PROPERTIES:\n:ID: hijack\n:END:".into()),
    );
    let (text, back) = round_trip(b);
    eprintln!("--- hazard 1 file ---\n{text}---");
    let back = back.unwrap_or_else(|| panic!("hazard 1: block:kid lost its id:\n{text}"));
    assert_eq!(
        back.get_property("note"),
        Some(Value::String(
            "line one\n* Evil heading\n:PROPERTIES:\n:ID: hijack\n:END:".into()
        )),
        "hazard 1: note did not round-trip:\n{text}"
    );
}

#[test]
#[ignore = "known red: the headline tag split eats a title that ends in colons"]
fn hazard2_title_ending_in_colons_before_tags_loses_text() {
    let mut failures = Vec::new();
    for (title, tags) in [
        ("Pick ::", vec!["decision"]),
        ("Pick :", vec!["decision"]),
        ("Pick:", vec!["decision"]),
        ("a ::", vec!["t"]),
        ("Meeting :urgent:", vec![]),
        ("Meeting :urgent:", vec!["later"]),
        ("Ratio 1:2:", vec!["t"]),
    ] {
        let mut b = kid(title);
        b.set_tags(Tags::from(
            tags.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        ));
        let want_tags = b.tags.to_set();
        let (text, back) = round_trip(b);
        let back = back.unwrap_or_else(|| panic!("{title:?}: lost id:\n{text}"));
        let got = (back.content.clone(), back.tags.to_set());
        eprintln!(
            "{title:?} + {tags:?} -> headline {:?} -> {:?}",
            text.lines().find(|l| l.starts_with("** ")),
            got
        );
        if got != (title.to_string(), want_tags.clone()) {
            failures.push(format!("{title:?}+{tags:?} -> {got:?}"));
        }
    }
    assert!(failures.is_empty(), "hazard 2 losses: {failures:#?}");
}

#[test]
#[ignore = "known red: typed property values are left out of the drawer"]
fn hazard3_non_text_property_values_vanish_from_the_file() {
    let mut failures = Vec::new();
    for value in [
        Value::Integer(3),
        Value::Float(1.5),
        Value::Boolean(true),
        Value::Json(r#"{"a":1}"#.into()),
        Value::Object([("a".to_string(), Value::String("b".into()))].into()),
        Value::Array(vec![Value::String("x".into())]),
        Value::DateTime("2026-01-01T00:00:00Z".into()),
    ] {
        let mut b = kid("Kid");
        b.set_property("note", value.clone());
        let (text, back) = round_trip(b);
        let back = back.expect("id survives");
        let got = back.get_property("note");
        eprintln!(
            "{value:?} -> in file: {} -> reparsed {got:?}",
            text.contains(":note:")
        );
        if got.is_none() {
            failures.push(format!("{value:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "hazard 3: values erased by the renderer: {failures:#?}"
    );
}

#[test]
fn related_text_values_that_do_not_round_trip() {
    let mut failures = Vec::new();
    for value in [" padded ", "", "\"quoted\"", "tab\there"] {
        let mut b = kid("Kid");
        b.set_property("note", Value::String(value.into()));
        let (text, back) = round_trip(b);
        let got = back.as_ref().map(|b| b.get_property("note"));
        eprintln!("note={value:?} -> id kept: {} -> {got:?}", back.is_some());
        if got != Some(Some(Value::String(value.into()))) {
            failures.push(format!("note={value:?} -> {got:?}\n{text}"));
        }
    }
    assert!(failures.is_empty(), "related losses: {failures:#?}");
}

#[test]
fn a_key_org_cannot_hold_leaves_the_rest_of_the_block_intact() {
    let mut failures = Vec::new();
    for key in [
        "a b",
        "a:b",
        "k\n* Evil",
        "END",
        "PROPERTIES",
        "",
        "tab\tkey",
    ] {
        let mut b = kid("Kid");
        b.set_property(key, Value::String("v".into()));
        b.set_property("kept", Value::String("yes".into()));
        let (text, back) = round_trip(b);
        let kept = back.as_ref().map(|b| b.get_property("kept"));
        eprintln!("{key:?} -> id kept: {} -> kept={kept:?}", back.is_some());
        if kept != Some(Some(Value::String("yes".into()))) {
            failures.push(format!("{key:?} -> kept={kept:?}\n{text}"));
        }
    }
    assert!(
        failures.is_empty(),
        "bad keys destroyed the block: {failures:#?}"
    );
}

#[test]
fn a_drawer_carrier_without_an_id_keeps_the_block_id() {
    let mut b = kid("Kid");
    b.set_org_properties(Some(r#"{"note":"v"}"#.to_string()));
    let (text, back) = round_trip(b);
    let back = back.unwrap_or_else(|| panic!("block:kid lost its id:\n{text}"));
    assert_eq!(
        back.get_property("note"),
        Some(Value::String("v".into())),
        "{text}"
    );
}
