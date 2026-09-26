//! The renderer writes only text property values into a `:PROPERTIES:`
//! drawer. A typed value that holds a line break never reaches the file, so
//! it cannot open a heading or close the drawer.

use std::path::Path;

use holon_api::EntityUri;
use holon_api::Value;
use holon_api::block::Block;
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

#[test]
fn a_typed_value_with_a_line_break_leaves_the_drawer_intact() {
    let injection = "a\n* Evil heading\n:END:";
    let typed = [
        Value::Json(serde_json::json!({ "t": injection }).to_string()),
        Value::Json("{\n\"t\": 1\n}".into()),
        Value::Object([("t".to_string(), Value::String(injection.into()))].into()),
        Value::Array(vec![Value::String(injection.into())]),
        Value::DateTime(format!("2026-01-01{injection}")),
    ];
    for value in typed {
        let (document, mut blocks) = parse("#+ID: p\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n");
        let mut kid = Block::new_text(
            EntityUri::block("kid"),
            EntityUri::block("topic"),
            "Kid".to_string(),
        );
        kid.set_property("note", value.clone());
        blocks.push(kid);
        let text = OrgRenderer::render_document(&document, &blocks, Path::new(FILE), &document.id);
        assert!(
            !text.contains("Evil heading"),
            "{value:?} reached the file:\n{text}"
        );
        assert!(
            !text.contains(":note:"),
            "{value:?} reached the drawer:\n{text}"
        );
        let (_, reparsed) = parse(&text);
        assert!(
            reparsed.iter().any(|b| b.id.as_str() == "block:kid"),
            "{value:?}: the block lost its id:\n{text}"
        );
    }
}
