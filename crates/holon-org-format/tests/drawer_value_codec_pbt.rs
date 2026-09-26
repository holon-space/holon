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

fn try_render(document: &Block, blocks: &[Block]) -> anyhow::Result<String> {
    OrgRenderer::render_document(document, blocks, Path::new(FILE), &document.id)
}

fn render(document: &Block, blocks: &[Block]) -> String {
    try_render(document, blocks).unwrap_or_else(|e| panic!("org render: {e:#}"))
}

fn skeleton() -> (Block, Vec<Block>) {
    parse("#+ID: p\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n")
}

/// A key that is one plain token always round-trips.
fn plain_key(k: &str) -> bool {
    k.starts_with('k')
        && k.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn key() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => "k[a-z0-9-]{0,6}",
        1 => prop_oneof![
            Just("ID"),
            Just("id"),
            Just("Id"),
            Just("iD"),
            Just("x+"),
            Just("+"),
            Just("PROPERTIES"),
            Just("properties"),
            Just("END"),
            Just("end"),
            Just("a b"),
            Just("a:b"),
            Just(""),
            Just("k\n* Evil"),
        ]
        .prop_map(str::to_string),
    ]
}

/// A key Holon's file-drawer line reader reads back as itself: one token
/// with no whitespace and no `:`, not a drawer delimiter.
fn file_reader_key(k: &str) -> bool {
    !k.is_empty()
        && !k.chars().any(|c| c.is_whitespace() || c == ':')
        && !["PROPERTIES", "END"]
            .iter()
            .any(|d| k.eq_ignore_ascii_case(d))
}

/// A key the source-block header-argument reader reads back as itself: one
/// whitespace-free token after the `:`, and not `id`, which names the block.
fn header_reader_key(k: &str) -> bool {
    !k.is_empty() && !k.chars().any(char::is_whitespace) && k != "id"
}

fn file_key() -> impl Strategy<Value = String> {
    prop_oneof![
        2 => key(),
        2 => "[a-zA-Z0-9_+*#.,=\u{e9}\u{1b}-]{1,8}",
        1 => prop_oneof![Just("note+"), Just("*k"), Just("#+k"), Just("k+v")]
            .prop_map(str::to_string),
    ]
}

/// The property keys the parser gives `kid` with nothing set on it.
fn parse_kid(document: &Block, blocks: &[Block], kid: Block) -> Vec<String> {
    let mut blocks = blocks.to_vec();
    blocks.push(kid);
    let (_, back) = parse(&render(document, &blocks));
    let kid = back
        .iter()
        .find(|b| b.id.id() == "kid")
        .expect("a plain kid round-trips");
    kid.properties.keys().cloned().collect()
}

/// The block an `:ID:` line with this value reads back as, when the value is
/// a bare block id the renderer writes.
fn carried_id(v: &str) -> Option<EntityUri> {
    holon_org_format::DrawerId::parse(v)
        .is_ok()
        .then(|| EntityUri::block(v))
}

/// The file's lines of the form `:<k>:` or `:<k>: ...`, so a key the renderer
/// left out is visibly absent rather than silently misread.
fn has_line_for(file: &str, k: &str) -> bool {
    let head = format!(":{k}:");
    let delimiter = k.eq_ignore_ascii_case("PROPERTIES") || k.eq_ignore_ascii_case("END");
    file.lines()
        .any(|l| (l == head && !delimiter) || l.starts_with(&format!("{head} ")))
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
        let baseline = parse_kid(&document, &blocks, kid.clone());
        kid.set_property(&k, Value::String(v.clone()));
        blocks.push(kid);
        let written_id = (k == "ID").then(|| carried_id(&v)).flatten();
        if k == "ID" && written_id.is_none() {
            let refused = try_render(&document, &blocks).map_err(|e| format!("{e:#}"));
            prop_assert!(
                refused.as_ref().is_err_and(|e| e.contains(&format!("{v:?}"))),
                "an id the :ID: line cannot carry must fail the render by name: {:?}",
                refused
            );
            return Ok(());
        }
        let text = render(&document, &blocks);
        let (_, back) = parse(&text);
        prop_assert_eq!(back.len(), 2, "the value grew or ate blocks:\n{}", text);
        let id = written_id.clone().unwrap_or_else(|| EntityUri::block("kid"));
        let kid = back.iter().find(|b| b.id == id);
        prop_assert!(kid.is_some(), "{} lost its id:\n{}", id, text);
        let kid = kid.unwrap();
        if plain_key(&k) || written_id.is_some() {
            prop_assert_eq!(kid.get_property(&k), Some(Value::String(v)), "file:\n{}", text);
        } else if k != "ID" {
            prop_assert!(!has_line_for(&text, &k), "key {:?} was written:\n{}", k, text);
        }
        let stray: Vec<String> = kid
            .properties
            .keys()
            .filter(|p| !p.starts_with('_') && !baseline.contains(*p) && p.as_str() != "ID" && **p != k)
            .cloned()
            .collect();
        prop_assert!(stray.is_empty(), "key {:?} read back as {:?}:\n{}", k, stray, text);
    }

    #[test]
    fn file_drawer_value_round_trips(k in file_key(), v in value()) {
        let (mut document, blocks) = skeleton();
        let mut drawer = serde_json::Map::new();
        drawer.insert(k.clone(), serde_json::Value::String(v.clone()));
        document.set_file_drawer(Some(drawer));
        let text = render(&document, &blocks);
        let (back, back_blocks) = parse(&text);
        prop_assert_eq!(back_blocks.len(), 1, "the value grew or ate blocks:\n{}", text);
        prop_assert_eq!(&back.id, &document.id, "the document lost its id:\n{}", text);
        let got = back.file_drawer().and_then(|d| d.get(&k).cloned());
        if k.eq_ignore_ascii_case("ID") {
            let want = if v.is_empty() { v } else { document.id.id().to_string() };
            prop_assert_eq!(got, Some(serde_json::Value::String(want)), "file:\n{}", text);
        } else if file_reader_key(&k) {
            prop_assert_eq!(got, Some(serde_json::Value::String(v)), "file:\n{}", text);
        } else {
            prop_assert!(!has_line_for(&text, &k), "key {:?} was written:\n{}", k, text);
        }
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
        prop_assert_eq!(back.len(), 2, "the value grew or ate blocks:\n{}", text);
        let src = back.iter().find(|b| b.id.id() == "src");
        prop_assert!(src.is_some(), "block:src lost its id:\n{}", text);
        if header_reader_key(&k) && k != "ID" {
            prop_assert_eq!(
                src.unwrap().get_property(&k),
                Some(Value::String(v)),
                "file:\n{}", text
            );
        } else {
            let head = format!(" :{k} ");
            let own_id_line = usize::from(k == "id");
            prop_assert_eq!(text.matches(&head).count(), own_id_line, "key {:?} was written:\n{}", k, text);
        }
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
    fn an_authored_file_drawer_is_written_back_byte_equal(
        k in file_key().prop_filter("the reader keeps a plain key", |k| {
            file_reader_key(k) && !k.eq_ignore_ascii_case("ID")
        }),
        t in typed_value(),
    ) {
        let source = format!(":PROPERTIES:\n:ID: p\n:{k}: {t}\n:END:\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n");
        let (document, blocks) = parse(&source);
        prop_assert_eq!(render(&document, &blocks), source);
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
