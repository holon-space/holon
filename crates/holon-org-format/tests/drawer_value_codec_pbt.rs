//! Every text property value survives render -> parse byte-equal, on every
//! org carrier of a property: the headline drawer, the file-level drawer and
//! a source block's header arguments. And a value an external editor typed
//! into a drawer line is written back exactly as typed.

use std::path::Path;

use holon_api::EntityUri;
use holon_api::Value;
use holon_api::block::Block;
use holon_org_format::OrgDocumentExt;
use holon_org_format::OrgRenderer;
use holon_org_format::ValueCarrier;
use holon_org_format::parse_org_file;
use proptest::prelude::*;

const FILE: &str = "/vault/p.org";

fn parse(source: &str) -> (Block, Vec<Block>) {
    let parsed = parse_org_file(
        Path::new(FILE),
        source,
        &EntityUri::no_parent(),
        Path::new("/vault"),
    )
    .unwrap_or_else(|e| panic!("parse failed: {e:#}\n{source}"));
    (parsed.document, parsed.blocks)
}

fn render(document: &Block, blocks: &[Block]) -> String {
    OrgRenderer::render_document(document, blocks, Path::new(FILE), &document.id)
}

fn skeleton() -> (Block, Vec<Block>) {
    parse("#+ID: p\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n")
}

fn key() -> impl Strategy<Value = String> {
    "k[a-z0-9-]{0,6}"
}

fn value() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        Just("\n"),
        Just("\r"),
        Just("\r\n"),
        Just(" "),
        Just("\t"),
        Just("\""),
        Just("\\"),
        Just(":END:"),
        Just(":PROPERTIES:"),
        Just("* "),
        Just("#+"),
        Just(":k "),
        Just("a"),
        Just("word"),
        Just("\u{e9}"),
        Just("\u{a0}"),
        Just("\u{2028}"),
        Just("\u{1f600}"),
    ];
    prop_oneof![
        3 => prop::collection::vec(piece, 0..8).prop_map(|p| p.concat()),
        1 => any::<String>(),
    ]
}

/// A drawer value as a person types it: one line, no surrounding space.
fn typed_value() -> impl Strategy<Value = String> {
    "[\"a-zA-Z0-9 :#*\\\\{}\\[\\]\u{e9}-]{1,24}"
        .prop_filter("a drawer line holds a trimmed value", |s| {
            s.trim() == s && !s.is_empty()
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn the_codecs_are_lossless(v in value()) {
        for carrier in [
            ValueCarrier::HeadlineDrawer,
            ValueCarrier::FileDrawer,
            ValueCarrier::HeaderArg,
        ] {
            let text = carrier.encode(&v);
            prop_assert_eq!(carrier.decode(&text), v.as_str(), "{:?} wrote {:?}", carrier, text);
        }
    }

    #[test]
    fn headline_drawer_value_round_trips(k in key(), v in value()) {
        let (document, mut blocks) = skeleton();
        let mut kid = Block::new_text(
            EntityUri::block("kid"),
            EntityUri::block("topic"),
            "Kid".to_string(),
        );
        kid.set_property(&k, Value::String(v.clone()));
        blocks.push(kid);
        let text = render(&document, &blocks);
        let (_, back) = parse(&text);
        let kid = back.iter().find(|b| b.id.id() == "kid");
        prop_assert!(kid.is_some(), "block:kid lost its id:\n{}", text);
        prop_assert_eq!(
            kid.unwrap().get_property(&k),
            Some(Value::String(v)),
            "file:\n{}", text
        );
        prop_assert_eq!(back.len(), 2, "the value grew or ate blocks:\n{}", text);
    }

    #[test]
    fn file_drawer_value_round_trips(k in key(), v in value()) {
        let (mut document, blocks) = skeleton();
        let mut drawer = serde_json::Map::new();
        drawer.insert(k.clone(), serde_json::Value::String(v.clone()));
        document.set_file_drawer(Some(drawer));
        let text = render(&document, &blocks);
        let (back, back_blocks) = parse(&text);
        let got = back.file_drawer().and_then(|d| d.get(&k).cloned());
        prop_assert_eq!(got, Some(serde_json::Value::String(v)), "file:\n{}", text);
        prop_assert_eq!(back_blocks.len(), 1, "the value grew or ate blocks:\n{}", text);
    }

    #[test]
    fn source_header_arg_value_round_trips(k in key(), v in value()) {
        let (document, mut blocks) = skeleton();
        let mut src = Block::new_source(
            EntityUri::block("src"),
            EntityUri::block("topic"),
            "python",
            "print(1)",
        );
        src.set_property(&k, Value::String(v.clone()));
        blocks.push(src);
        let text = render(&document, &blocks);
        let (_, back) = parse(&text);
        let src = back.iter().find(|b| b.id.id() == "src");
        prop_assert!(src.is_some(), "block:src lost its id:\n{}", text);
        prop_assert_eq!(
            src.unwrap().get_property(&k),
            Some(Value::String(v)),
            "file:\n{}", text
        );
    }

    #[test]
    fn a_typed_headline_drawer_value_is_written_back_as_typed(t in typed_value()) {
        let source = format!("#+ID: p\n* Topic\n:PROPERTIES:\n:ID: topic\n:note: {t}\n:END:\n");
        let (document, blocks) = parse(&source);
        let text = render(&document, &blocks);
        let line = format!(":note: {t}\n");
        prop_assert!(text.contains(&line), "{:?} was rewritten:\n{}", t, text);
    }

    #[test]
    fn a_typed_file_drawer_value_is_written_back_as_typed(t in typed_value()) {
        let source = format!(":PROPERTIES:\n:ID: p\n:note: {t}\n:END:\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n");
        let (document, blocks) = parse(&source);
        let text = render(&document, &blocks);
        let line = format!(":note: {t}\n");
        prop_assert!(text.contains(&line), "{:?} was rewritten:\n{}", t, text);
    }
}
