//! An id a Markdown file declares (LogSeq `id::`, an Obsidian `^anchor`) is
//! taken as the same bare block id or the file is refused by name, as for an
//! org `:ID:`.

use std::path::Path;

use holon_api::EntityUri;
use holon_core::file_format::FileFormatAdapter;
use holon_markdown::LogseqMarkdownAdapter;
use holon_markdown::ObsidianMarkdownAdapter;

const UNHOLDABLE: [&str; 6] = ["block:abc", "a b", "café", "a#b", ":x", "a%zz"];

fn parse(adapter: &dyn FileFormatAdapter, content: &str) -> anyhow::Result<Vec<EntityUri>> {
    let root = Path::new("/vault");
    adapter
        .parse(
            &root.join("Page.md"),
            content,
            &EntityUri::no_parent(),
            root,
        )
        .map(|r| {
            std::iter::once(r.document.id)
                .chain(r.blocks.into_iter().map(|b| b.id))
                .collect()
        })
}

fn assert_refused_by_name(id: &str, result: anyhow::Result<impl std::fmt::Debug>) {
    match result {
        Ok(ids) => panic!("id {id:?} was accepted as {ids:?}"),
        Err(e) => {
            let msg = format!("{e:#}");
            assert!(
                msg.contains(&format!("{id:?}")),
                "the refusal names {id:?}: {msg}"
            );
        }
    }
}

#[test]
fn a_logseq_page_id_no_carrier_holds_refuses_the_file() {
    for id in UNHOLDABLE {
        let content = format!("id:: {id}\n\n- a block\n");
        assert_refused_by_name(id, parse(&LogseqMarkdownAdapter::new(), &content));
        assert_refused_by_name(
            id,
            LogseqMarkdownAdapter::new().doc_id_from_content(&content),
        );
    }
}

#[test]
fn a_logseq_block_id_no_carrier_holds_refuses_the_file() {
    for id in UNHOLDABLE {
        let content = format!("- a block\n  id:: {id}\n");
        assert_refused_by_name(id, parse(&LogseqMarkdownAdapter::new(), &content));
    }
}

#[test]
fn an_obsidian_anchor_no_carrier_holds_refuses_the_file() {
    let content = "a paragraph ^café\n";
    assert_refused_by_name("café", parse(&ObsidianMarkdownAdapter::new(), content));
}

#[test]
fn a_holdable_declared_id_is_taken_as_itself() {
    let uuid = "11111111-0000-4000-8000-000000000001";
    let logseq = parse(
        &LogseqMarkdownAdapter::new(),
        &format!("id:: {uuid}\n\n- a block\n  id:: kid-1\n"),
    )
    .unwrap_or_else(|e| panic!("{e:#}"));
    assert_eq!(
        logseq[..2],
        [EntityUri::block(uuid), EntityUri::block("kid-1")]
    );
    let obsidian = parse(&ObsidianMarkdownAdapter::new(), "a paragraph ^anchor-1\n")
        .unwrap_or_else(|e| panic!("{e:#}"));
    assert!(
        obsidian.contains(&EntityUri::block("anchor-1")),
        "{obsidian:?}"
    );
}
