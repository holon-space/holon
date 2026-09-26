//! A `#+TODO:` keyword spelled with org's fast-access key and log markers
//! (`NEXT(n)`, `WAIT(w@/!)`) is the keyword before the `(`, as org reads it
//! (org manual, "Fast access to TODO states" and "Tracking TODO state
//! changes").

use std::path::Path;

use holon_api::EntityUri;
use holon_api::TaskState;
use holon_api::block::Block;
use holon_org_format::OrgBlockExt;
use holon_org_format::OrgDocumentExt;
use holon_org_format::OrgRenderer;
use holon_org_format::parse_org_file;

const FILE: &str = "/vault/ring.org";

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

fn headline(blocks: &[Block], title: &str) -> (String, Option<TaskState>) {
    let block = blocks
        .iter()
        .find(|b| b.org_title().ends_with(title))
        .unwrap_or_else(|| panic!("no headline ending in {title:?}: {blocks:?}"));
    (block.org_title(), block.task_state())
}

#[test]
fn a_fast_access_key_is_not_part_of_the_keyword() {
    let (document, blocks) = parse("#+TODO: NEXT(n) | DONE(d)\n* NEXT Plan\n* NEXT(n) Spelled\n");
    assert_eq!(
        document.todo_keywords(),
        Some(vec![TaskState::active("NEXT"), TaskState::done("DONE")])
    );
    assert_eq!(
        headline(&blocks, "Plan"),
        ("Plan".to_string(), Some(TaskState::active("NEXT")))
    );
    assert_eq!(
        headline(&blocks, "Spelled"),
        ("NEXT(n) Spelled".to_string(), None),
        "org reads the header's spelling in a headline as title text"
    );
}

#[test]
fn log_markers_are_not_part_of_the_keyword() {
    let (document, blocks) =
        parse("#+TODO: TODO(t) WAIT(w@/!) | DONE(d!) CANCELED(c@)\n* WAIT Call\n* CANCELED Trip\n");
    assert_eq!(
        document.todo_keywords(),
        Some(vec![
            TaskState::active("TODO"),
            TaskState::active("WAIT"),
            TaskState::done("DONE"),
            TaskState::done("CANCELED"),
        ])
    );
    assert_eq!(
        headline(&blocks, "Call"),
        ("Call".to_string(), Some(TaskState::active("WAIT")))
    );
    assert_eq!(
        headline(&blocks, "Trip"),
        ("Trip".to_string(), Some(TaskState::done("CANCELED")))
    );
}

#[test]
fn a_ring_with_fast_access_keys_is_written_back_as_authored() {
    let source = "#+TODO: NEXT(n) WAIT(w@/!) | DONE(d!)\n* NEXT Plan\n";
    let (document, blocks) = parse(source);
    let written = OrgRenderer::render_document(&document, &blocks, Path::new(FILE), &document.id)
        .expect("org render")
        .text;
    assert!(
        written.contains("#+TODO: NEXT(n) WAIT(w@/!) | DONE(d!)\n"),
        "the header keeps its spelling:\n{written}"
    );
}

fn render(document: &Block, blocks: &[Block]) -> String {
    OrgRenderer::render_document(document, blocks, Path::new(FILE), &document.id)
        .expect("org render")
        .text
}

/// `org-remove-keyword-keys` strips `(.*)$`: a parenthesis group only at the
/// END of the word.
#[test]
fn a_parenthesis_that_does_not_end_the_word_is_part_of_the_keyword() {
    let (document, blocks) =
        parse("#+TODO: FOO(bar TO(DO)X | DONE\n* FOO(bar Plan\n* FOO Other\n* TO(DO)X Third\n");
    assert_eq!(
        document.todo_keywords(),
        Some(vec![
            TaskState::active("FOO(bar"),
            TaskState::active("TO(DO)X"),
            TaskState::done("DONE"),
        ])
    );
    assert_eq!(
        headline(&blocks, "Plan"),
        ("Plan".to_string(), Some(TaskState::active("FOO(bar")))
    );
    assert_eq!(headline(&blocks, "Other"), ("FOO Other".to_string(), None));
    assert_eq!(
        headline(&blocks, "Third"),
        ("Third".to_string(), Some(TaskState::active("TO(DO)X")))
    );
}

/// A `#+TODO:` line org does not read as a file keyword declares nothing and
/// is not written back as a header.
#[test]
fn a_todo_line_that_is_no_file_keyword_declares_no_ring() {
    for (shape, source) in [
        (
            "inside a preamble block",
            "#+BEGIN_EXAMPLE\n#+TODO: LATER | DONE\n#+END_EXAMPLE\n* TODO Plan\n",
        ),
        (
            "below a headline",
            "* Notes\n,#+TODO: LATER | DONE\n* TODO Plan\n",
        ),
    ] {
        let (document, blocks) = parse(source);
        assert_eq!(document.todo_keywords(), None, "{shape}");
        assert_eq!(
            headline(&blocks, "Plan"),
            ("Plan".to_string(), Some(TaskState::active("TODO"))),
            "{shape}"
        );
        let written = render(&document, &blocks);
        assert_eq!(
            written.matches("#+TODO:").count(),
            source.matches("#+TODO:").count(),
            "{shape}: no ring header is written:\n{written}"
        );
    }
}

/// Org reads every `#+TODO:`, `#+SEQ_TODO:` and `#+TYP_TODO:` line, the
/// `#+TYP_TODO:` lines first; a line with no `|` makes its last word the done
/// keyword.
#[test]
fn every_ring_line_declares_keywords() {
    let source = "#+TODO: NEXT | DONE\n#+TYP_TODO: ALPHA OMEGA\n* ALPHA a\n* OMEGA b\n* NEXT c\n";
    let (document, blocks) = parse(source);
    assert_eq!(
        document.todo_keywords(),
        Some(vec![
            TaskState::active("ALPHA"),
            TaskState::done("OMEGA"),
            TaskState::active("NEXT"),
            TaskState::done("DONE"),
        ])
    );
    assert_eq!(
        headline(&blocks, "a"),
        ("a".to_string(), Some(TaskState::active("ALPHA")))
    );
    assert_eq!(
        headline(&blocks, "b"),
        ("b".to_string(), Some(TaskState::done("OMEGA")))
    );
    let written = render(&document, &blocks);
    assert!(
        written.starts_with("#+TODO: NEXT | DONE\n#+TYP_TODO: ALPHA OMEGA\n"),
        "both lines are written back as authored:\n{written}"
    );
}
