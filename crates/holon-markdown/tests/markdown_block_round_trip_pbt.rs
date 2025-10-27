//! Block-id round-trip PBT through the **Markdown format adapter**.
//!
//! Sister test to `holon-orgmode/tests/org_block_round_trip_pbt.rs`. Org has
//! carried a block round-trip property for a while; markdown was the odd one
//! out (H6 in `docs/Architecture/BlockEventStorm.md`). This test pins the
//! headline property:
//!
//! ```text
//! parse(render(block)).id == block.id
//! ```
//!
//! Two halves, matching the renderer's contract:
//!
//! 1. **Valid-charset ids round-trip identically.** Any id in `[A-Za-z0-9_-]`
//!    (UUIDs included) survives `render_blocks → parse` with its id intact and
//!    triggers no writeback remint.
//! 2. **Out-of-charset ids fail loudly.** An id with a character outside the
//!    round-trip-safe charset used to be silently dropped to an empty `^`
//!    marker — the reparse then minted a *fresh* UUID, losing the block's
//!    identity with no signal. The renderer now returns
//!    [`MarkdownRenderError::OutOfCharsetBlockId`] instead, and this test pins
//!    that (no silent remint).
//!
//! The property drives the **inherent** [`MarkdownRenderer`] (which returns
//! `Result`) rather than the `FileFormatAdapter` trait method (which returns
//! `String` and panics on the error), so the loud-error half is observable.
//!
//! Latency: markdown is not yet wired into file-sync (`holon-markdown` has zero
//! prod dependents). When it is, this property graduates into the composed
//! invariant catalog per the `pbt-composition` skill — see the H6 row.

use std::path::PathBuf;

use holon_api::EntityUri;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::types::ContentType;
use holon_core::file_format::FileFormatAdapter;
use holon_markdown::MarkdownDialect;
use holon_markdown::MarkdownFormatAdapter;
use holon_markdown::MarkdownRenderError;
use holon_markdown::MarkdownRenderer;
use proptest::prelude::*;

/// A non-empty heading title: a letter followed by letters/spaces, so it never
/// trims to empty and never carries a stray `^` that a reparse could mistake
/// for a block-id marker. Title fidelity is not under test here — only the id.
fn title_strategy() -> impl Strategy<Value = String> {
    "[a-zA-Z][a-zA-Z ]{0,19}".prop_map(|s| s)
}

/// A round-trip-safe id core: the exact charset the renderer + parser agree on.
fn id_core_strategy() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_-]{1,12}".prop_map(|s| s)
}

/// Characters outside `[A-Za-z0-9_-]` that are still valid inside an RFC-3986
/// URI path (so `EntityUri::block` can hold them without panicking). This is
/// the realistic hand-authored / punctuated-id case the renderer must reject.
fn out_of_charset_char() -> impl Strategy<Value = char> {
    prop::sample::select(vec![
        '.', '~', '!', '$', '(', ')', '*', '+', ',', ';', '=', '@',
    ])
}

fn heading_block(id: &str, parent: &EntityUri, title: &str) -> Block {
    let mut b = Block::new_text(EntityUri::block(id), parent.clone(), title.to_string());
    b.set_property("ID", Value::String(id.to_string()));
    b
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    /// Valid-charset ids survive render → parse unchanged, in order, with no
    /// writeback remint.
    #[test]
    fn valid_charset_ids_round_trip(
        specs in prop::collection::vec((title_strategy(), id_core_strategy()), 1..8),
    ) {
        let root_id = EntityUri::block("round-trip-doc");
        let mut blocks = Vec::new();
        let mut expected_ids = Vec::new();
        for (i, (title, core)) in specs.iter().enumerate() {
            // Prefix with the index to guarantee uniqueness; the prefix stays
            // in-charset so the id remains round-trip-safe.
            let id = format!("b{i}-{core}");
            blocks.push(heading_block(&id, &root_id, title));
            expected_ids.push(id);
        }

        let dialect = MarkdownDialect::obsidian();
        let path = PathBuf::from("/vault/note.md");
        let root = PathBuf::from("/vault");

        let text = MarkdownRenderer::new(dialect.clone())
            .render_blocks(&blocks, &path, &root_id)
            .map_err(|e| TestCaseError::fail(format!("valid ids must render: {e}")))?;

        let parsed = MarkdownFormatAdapter::with_dialect(dialect)
            .parse(&path, &text, &EntityUri::no_parent(), &root)
            .map_err(|e| TestCaseError::fail(format!("parse failed: {e}\n\n{text}")))?;

        let actual_ids: Vec<String> = parsed
            .blocks
            .iter()
            .filter(|b| matches!(b.content_type, ContentType::Text))
            .map(|b| b.id.id().to_string())
            .collect();

        prop_assert_eq!(&actual_ids, &expected_ids, "id round-trip mismatch\n\n{}", text);
        prop_assert!(
            parsed.blocks_needing_ids.is_empty(),
            "a round-trip-safe id was reminted (writeback recorded): {:?}\n\n{}",
            parsed.blocks_needing_ids,
            text
        );
    }

    /// An id with an out-of-charset character is a loud render error, never a
    /// silently dropped marker (which would remint a UUID on reparse).
    #[test]
    fn out_of_charset_id_is_a_loud_render_error(
        prefix in "[a-zA-Z0-9_-]{0,6}",
        bad in out_of_charset_char(),
        suffix in "[a-zA-Z0-9_-]{0,6}",
    ) {
        let id = format!("{prefix}{bad}{suffix}");
        let root_id = EntityUri::block("err-doc");
        let block = heading_block(&id, &root_id, "Heading");

        let path = PathBuf::from("/vault/note.md");
        let result = MarkdownRenderer::new(MarkdownDialect::obsidian())
            .render_blocks(&[block], &path, &root_id);

        match result {
            Err(MarkdownRenderError::OutOfCharsetBlockId { id: reported, offending }) => {
                prop_assert_eq!(&reported, &id);
                // `prefix` is in-charset, so the first out-of-charset char is `bad`.
                prop_assert_eq!(offending, bad);
            }
            other => prop_assert!(
                false,
                "out-of-charset id {id:?} must be a loud OutOfCharsetBlockId error, got {other:?}"
            ),
        }
    }
}

/// An empty id has nothing to anchor a `^id` marker to — also a loud error,
/// never a silent empty marker.
#[test]
fn empty_block_id_is_a_loud_render_error() {
    let root_id = EntityUri::block("err-doc");
    let block = heading_block("", &root_id, "Heading");
    let err = MarkdownRenderer::new(MarkdownDialect::obsidian())
        .render_blocks(&[block], &PathBuf::from("/vault/note.md"), &root_id)
        .expect_err("empty block id must fail loudly");
    assert_eq!(err, MarkdownRenderError::EmptyBlockId);
}
