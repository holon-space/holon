//! The id contract of every org id carrier — a headline's `:ID:`, a source
//! block's `:id`, and a page's `#+ID:` / file-drawer `:ID:`: an id the file
//! can carry is written and reads back as the SAME id; any other id is
//! refused by name, on write and on read.

use std::path::Path;

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_org_format::DrawerId;
use holon_org_format::OrgRenderer;
use holon_org_format::parse_org_file;
use proptest::prelude::*;

const FILE: &str = "/vault/p.org";

fn try_parse(source: &str) -> anyhow::Result<holon_org_format::ParseResult> {
    parse_org_file(
        Path::new(FILE),
        source,
        &EntityUri::no_parent(),
        Path::new("/vault"),
    )
}

fn page() -> Block {
    try_parse("#+ID: page-under-test\n")
        .unwrap_or_else(|e| panic!("{e:#}"))
        .document
}

fn render(document: &Block, blocks: &[Block]) -> anyhow::Result<String> {
    OrgRenderer::render_document(document, blocks, Path::new(FILE), &document.id).map(|r| r.text)
}

#[derive(Debug, Clone, Copy)]
enum Carrier {
    Headline,
    Source,
}

fn kid(carrier: Carrier, id: EntityUri, parent: &Block) -> Block {
    match carrier {
        Carrier::Headline => Block::new_text(id, parent.id.clone(), "kid"),
        Carrier::Source => Block::new_source(id, parent.id.clone(), "python", "print(1)"),
    }
}

/// The id `block:<id>` comes back as after render -> parse, on `carrier`.
fn round_trip(carrier: Carrier, id: &str) -> EntityUri {
    let doc = page();
    let uri = EntityUri::block(id);
    let file = render(&doc, &[kid(carrier, uri.clone(), &doc)])
        .unwrap_or_else(|e| panic!("{carrier:?} render of {id:?} refused: {e:#}"));
    let parsed = try_parse(&file).unwrap_or_else(|e| panic!("{carrier:?} {id:?}: {e:#}\n{file}"));
    assert_eq!(parsed.blocks.len(), 1, "{carrier:?} {id:?}:\n{file}");
    parsed.blocks[0].id.clone()
}

/// The ids production mints for blocks and pages.
fn minted_id() -> impl Strategy<Value = String> {
    let segment = "[A-Za-z0-9 ._()äé&+=-]{1,10}";
    prop_oneof![
        // A UUID: engine creates, split_block, org ingest of an id-less heading.
        any::<u128>().prop_map(uuid_text),
        // Markdown `<relpath>::b::<n>` (obsidian.rs / logseq.rs `mint`), with
        // subdirectories: the relpath is `EntityUri::file`'s encoded path.
        (prop::collection::vec(segment, 1..4), 0usize..500).prop_map(|(segments, n)| {
            let rel = format!("{}.md", segments.join("/"));
            format!("{}::b::{n}", EntityUri::file(&rel).id())
        }),
        // Org source block without `:id`: `<parent>::src::<n>`, whose parent
        // may be a name-chain page's path.
        (prop::collection::vec(segment, 1..3), 0usize..20).prop_map(|(segments, n)| {
            let rel = format!("{}.org", segments.join("/"));
            format!("{}::src::{n}", EntityUri::file(&rel).id())
        }),
        // Loro tree ids `{peer}:{counter}`.
        (any::<u64>(), 0i32..i32::MAX).prop_map(|(peer, counter)| EntityUri::block_from_tree_id(
            peer, counter
        )
        .id()
        .to_string()),
        // Slugs: journal dates and layout ids.
        "[a-z][a-z0-9-]{0,30}",
        "[a-z][a-z0-9-]{0,12}::(src|render)::[0-9]{1,3}",
        "20[0-9]{2}-[01][0-9]-[0-3][0-9]",
    ]
}

fn uuid_text(n: u128) -> String {
    let h = format!("{n:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// Ids no carrier holds as the same id.
fn unwritable_id() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        // whitespace or a control character inside
        (
            "[a-z]{1,5}",
            prop_oneof![Just(" "), Just("\t"), Just("\n"), Just("\u{7}")],
            "[a-z]{1,5}"
        )
            .prop_map(|(a, gap, b)| format!("{a}{gap}{b}")),
        // longer than an `:ID:` line holds
        (DrawerId::MAX_LEN + 1..DrawerId::MAX_LEN + 40).prop_map(|n| "a".repeat(n)),
        // an id that names a scheme of its own
        (
            prop_oneof![Just("block"), Just("doc"), Just("file"), Just("sentinel")],
            "[a-z0-9-]{1,10}"
        )
            .prop_map(|(scheme, id)| format!("{scheme}:{id}")),
        // a leading `:` starts a new header argument in a source block
        "[a-z0-9-]{1,10}".prop_map(|id| format!(":{id}")),
    ]
}

/// Any text, weighted towards what a URI or an org line treats specially.
fn any_text() -> impl Strategy<Value = String> {
    prop_oneof![
        "[ -~]{1,12}",
        "[a-z0-9#?%&=+:/. -]{1,10}",
        "(?s).{1,6}",
        "[a-z]{1,4}[\\x00-\\x1f\\x7f\u{a0}\u{2028}][a-z]{0,4}",
    ]
}

#[derive(Debug, Clone, Copy)]
enum AnyCarrier {
    Block(Carrier),
    Page,
}

const ALL_CARRIERS: [AnyCarrier; 3] = [
    AnyCarrier::Block(Carrier::Headline),
    AnyCarrier::Block(Carrier::Source),
    AnyCarrier::Page,
];

#[derive(Debug, Clone, PartialEq)]
enum Written {
    Refused,
    Same,
    Rewritten(String),
}

/// What `id` comes back as after render -> parse on `carrier`.
fn write_and_read(carrier: AnyCarrier, id: &EntityUri) -> Written {
    let mut doc = page();
    let rendered = match carrier {
        AnyCarrier::Block(c) => render(&doc, &[kid(c, id.clone(), &doc)]),
        AnyCarrier::Page => {
            doc.id = id.clone();
            render(&doc, &[])
        }
    };
    let Ok(file) = rendered else {
        return Written::Refused;
    };
    let read = match try_parse(&file) {
        Ok(parsed) => match carrier {
            AnyCarrier::Block(_) => parsed.blocks.first().map(|b| b.id.clone()),
            AnyCarrier::Page => Some(parsed.document.id),
        },
        Err(e) => return Written::Rewritten(format!("unreadable: {e:#}\n{file}")),
    };
    match read {
        Some(read) if &read == id => Written::Same,
        other => Written::Rewritten(format!("{other:?}\n{file}")),
    }
}

proptest! {
    #[test]
    fn any_text_is_accepted_exactly_when_every_carrier_keeps_it(raw in any_text()) {
        let accepted = DrawerId::parse(&raw).is_ok();
        match EntityUri::parse(&format!("block:{raw}")) {
            Err(_) => prop_assert!(!accepted, "{raw:?} forms no block URI but was accepted"),
            Ok(id) => {
                let expected = if accepted { Written::Same } else { Written::Refused };
                for carrier in ALL_CARRIERS {
                    prop_assert_eq!(write_and_read(carrier, &id), expected.clone(), "{:?} {}", carrier, id);
                }
            }
        }
    }

    #[test]
    fn every_minted_id_is_carried_as_itself(id in minted_id()) {
        prop_assert!(DrawerId::parse(&id).is_ok(), "{id:?} refused: {}", DrawerId::parse(&id).unwrap_err());
        for carrier in [Carrier::Headline, Carrier::Source] {
            prop_assert_eq!(round_trip(carrier, &id), EntityUri::block(&id), "{:?}", carrier);
        }
        let file = format!("#+ID: {id}\n* A\n");
        let doc = try_parse(&file).unwrap_or_else(|e| panic!("{e:#}")).document;
        prop_assert_eq!(&doc.id, &EntityUri::block(&id));
        let header = render(&doc, &[]).unwrap_or_else(|e| panic!("{e:#}"));
        let line = format!("#+ID: {id}\n");
        prop_assert!(header.contains(&line), "{}", header);
    }

    #[test]
    fn an_unwritable_id_is_refused_by_name(id in unwritable_id()) {
        let err = DrawerId::parse(&id).expect_err("an unwritable id is refused");
        prop_assert!(err.to_string().contains(&format!("{id:?}")), "{err}");
    }
}

/// A URI fragment or query is part of the id, so an id line cannot drop it.
#[test]
fn an_id_with_a_fragment_or_query_is_refused_on_every_carrier() {
    for raw in ["block:a#b", "block:a?b", "block:a#b?c"] {
        let id = EntityUri::parse(raw).unwrap();
        for carrier in ALL_CARRIERS {
            assert_eq!(
                write_and_read(carrier, &id),
                Written::Refused,
                "{carrier:?} {raw}"
            );
        }
    }
}

/// A block whose id no carrier holds is refused by name on BOTH block
/// carriers, never written without its scheme or split into a new key.
#[test]
fn a_block_id_no_carrier_holds_is_refused_on_render() {
    let doc = page();
    for raw in ["block::split-1", "doc:x", "file:x", "sentinel:no_parent"] {
        let uri = EntityUri::from_raw(raw);
        for carrier in [Carrier::Headline, Carrier::Source] {
            match render(&doc, &[kid(carrier, uri.clone(), &doc)]) {
                Ok(file) => panic!("{carrier:?} render of {raw} was accepted:\n{file}"),
                Err(e) => {
                    let msg = format!("{e:#}");
                    assert!(
                        msg.contains(raw) && msg.contains("refused"),
                        "{carrier:?} refusal must name {raw}: {msg}"
                    );
                }
            }
        }
    }
}

/// A source block's authored `:id` follows the heading `:ID:` rule.
#[test]
fn a_source_id_no_carrier_holds_refuses_the_file() {
    for id in ["doc:x", "block:x", "sentinel:no_parent", ":split-1"] {
        let file = format!("#+ID: p\n#+BEGIN_SRC python :id {id}\nprint(1)\n#+END_SRC\n");
        match try_parse(&file) {
            Ok(parsed) => panic!(
                "`:id {id}` was accepted as {:?}",
                parsed
                    .blocks
                    .iter()
                    .map(|b| b.id.as_str())
                    .collect::<Vec<_>>()
            ),
            Err(e) => {
                let msg = format!("{e:#}");
                assert!(msg.contains("source block"), "{msg}");
            }
        }
    }
}

/// Martin 2026-09-27: a page id takes bare block ids only, like a heading.
#[test]
fn a_page_id_that_names_a_scheme_refuses_the_file() {
    for file in [
        "#+ID: block:abc\n* A\n",
        "#+ID: doc:x\n* A\n",
        ":PROPERTIES:\n:ID: block:abc\n:END:\n* A\n",
        "#+ID: :x\n* A\n",
    ] {
        match try_parse(file) {
            Ok(parsed) => panic!("{file:?} was accepted as {}", parsed.document.id),
            Err(e) => {
                let msg = format!("{e:#}");
                assert!(
                    msg.contains("p.org") && msg.contains("document id"),
                    "{msg}"
                );
            }
        }
    }
}

/// A page whose id no `#+ID:` line holds is refused on render, not written
/// without its scheme or silently left to its path.
#[test]
fn a_page_id_no_carrier_holds_is_refused_on_render() {
    for raw in ["block::x", "doc:x", "sentinel:no_parent"] {
        let mut doc = page();
        doc.id = EntityUri::from_raw(raw);
        match render(&doc, &[]) {
            Ok(header) => panic!("page {raw} rendered:\n{header}"),
            Err(e) => {
                let msg = format!("{e:#}");
                assert!(msg.contains(raw) && msg.contains("refused"), "{msg}");
            }
        }
    }
}

/// A name-chain page keeps its path identity: no `#+ID:` line at all.
#[test]
fn a_name_chain_page_renders_without_an_id_line() {
    let doc = try_parse("* A\n")
        .unwrap_or_else(|e| panic!("{e:#}"))
        .document;
    assert!(doc.id.is_file(), "{}", doc.id);
    let header = render(&doc, &[]).unwrap_or_else(|e| panic!("{e:#}"));
    assert!(!header.contains("ID"), "{header}");
}
