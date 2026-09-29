//! Deeply nested authored text reads and writes back without taking the
//! process down: an unedited file keeps its bytes or the render names what it
//! changes, and block text holding the same shape reads back as it writes.

use std::path::Path;

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_org_format::OrgRenderer;
use holon_org_format::parse_org_file;
use proptest::prelude::*;

const FILE: &str = "/vault/p.org";
const HEAD: &str = "* H\n:PROPERTIES:\n:ID: h\n:END:\n";

fn parse(source: &str) -> (Block, Vec<Block>) {
    let parsed = parse_org_file(
        Path::new(FILE),
        source,
        &EntityUri::no_parent(),
        Path::new("/vault"),
    )
    .unwrap_or_else(|e| panic!("parse failed: {e:#}"));
    (parsed.document, parsed.blocks)
}

fn render(document: &Block, blocks: &[Block]) -> (String, usize) {
    let r = OrgRenderer::render_document(document, blocks, Path::new(FILE), &document.id)
        .unwrap_or_else(|e| panic!("render failed: {e:#}"));
    (r.text, r.losses.len())
}

#[derive(Debug, Clone, Copy)]
enum Shape {
    Markers(char),
    NestedEmphasis,
    NestedSuperscript,
    NestedFootnoteReferences,
    NestedLinkDescriptions,
    NestedQuoteBlocks,
    NestedList,
    OpenEmphasisRun,
}

fn nested(open: &str, close: &str, depth: usize) -> String {
    format!("{}x{}", open.repeat(depth), close.repeat(depth))
}

impl Shape {
    /// The text for `size`, scaled down for the shapes org itself reads in
    /// quadratic time.
    fn text(self, size: usize) -> String {
        match self {
            Shape::Markers(c) => c.to_string().repeat(size),
            Shape::NestedEmphasis => nested("*/_+", "+_/*", size / 8),
            Shape::NestedSuperscript => nested("a^{", "}", size / 4),
            Shape::NestedFootnoteReferences => nested("[fn::", "]", size / 6),
            Shape::NestedLinkDescriptions => nested("[[x][", "]]", size / 7),
            Shape::NestedQuoteBlocks => nested("#+begin_quote\n", "#+end_quote\n", size / 100),
            Shape::NestedList => (0..size / 4)
                .map(|i| format!("{}- x\n", " ".repeat(i)))
                .collect(),
            Shape::OpenEmphasisRun => "*a ".repeat(size / 30),
        }
    }
}

fn shape() -> impl Strategy<Value = Shape> {
    prop_oneof![
        prop::sample::select(vec!['*', '/', '+', '_']).prop_map(Shape::Markers),
        Just(Shape::NestedEmphasis),
        Just(Shape::NestedSuperscript),
        Just(Shape::NestedFootnoteReferences),
        Just(Shape::NestedLinkDescriptions),
        Just(Shape::NestedQuoteBlocks),
        Just(Shape::NestedList),
        Just(Shape::OpenEmphasisRun),
    ]
}

#[derive(Debug, Clone, Copy)]
enum Place {
    FirstLine,
    HeadlineBody,
    HeadlineTitle,
}

fn place(place: Place, text: &str) -> String {
    match place {
        Place::FirstLine => format!("{text}\n"),
        Place::HeadlineBody => format!("{HEAD}{text}\n"),
        Place::HeadlineTitle => format!(
            "* {}\n:PROPERTIES:\n:ID: t\n:END:\n",
            text.replace('\n', " ").trim_end()
        ),
    }
}

fn check(shape: Shape, size: usize, where_: Place) -> Result<(), TestCaseError> {
    let text = shape.text(size);
    let file = place(where_, &text);
    let (document, blocks) = parse(&file);
    let (written, losses) = render(&document, &blocks);
    prop_assert!(
        written == file || losses > 0,
        "{shape:?} x{size} {where_:?}: rewritten without a disclosed loss"
    );

    let (document, mut blocks) = parse(HEAD);
    blocks[0].content = format!("H\n{text}");
    let (written, losses) = render(&document, &blocks);
    let (document, back) = parse(&written);
    let (rewritten, _) = render(&document, &back);
    prop_assert!(
        rewritten == written || losses > 0,
        "{shape:?} x{size}: block text is no fixed point after one write"
    );
    Ok(())
}

#[test]
fn a_line_of_100k_bare_stars_reads_and_writes_back() {
    for where_ in [Place::FirstLine, Place::HeadlineBody, Place::HeadlineTitle] {
        check(Shape::Markers('*'), 100_000, where_).unwrap();
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 10, ..ProptestConfig::default() })]

    #[test]
    fn deeply_nested_text_reads_and_writes_back(
        shape in shape(),
        size in 2_000usize..8_000,
        where_ in prop_oneof![Just(Place::FirstLine), Just(Place::HeadlineBody), Just(Place::HeadlineTitle)],
    ) {
        check(shape, size, where_)?;
    }
}
