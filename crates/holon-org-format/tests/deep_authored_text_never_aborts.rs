//! Deeply nested authored text reads and writes back without taking the
//! process down: an unedited file keeps its bytes or the render names what it
//! changes, and block text holding the same shape reads back as it writes.

use std::alloc::GlobalAlloc;
use std::alloc::Layout;
use std::alloc::System;
use std::cell::Cell;
use std::path::Path;

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_org_format::OrgRenderer;
use holon_org_format::extract_inline_marks;
use holon_org_format::parse_org_file;
use holon_org_format::render_lossless;
use proptest::prelude::*;

/// Counts the bytes this thread allocates, so a test can tell linear from
/// quadratic work by a count rather than by a clock.
struct CountingAllocator;

thread_local! {
    static ALLOCATED: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATED.with(|a| a.set(a.get() + layout.size()));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATED.with(|a| a.set(a.get() + new_size.saturating_sub(layout.size())));
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

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
    ClosedEmphasisRun,
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
            Shape::ClosedEmphasisRun => "*/a/* ".repeat(size / 6),
        }
    }
}

const MARKERS: [char; 9] = ['*', '/', '+', '_', '[', '{', '(', '~', '='];

fn shape() -> impl Strategy<Value = Shape> {
    prop_oneof![
        prop::sample::select(MARKERS.to_vec()).prop_map(Shape::Markers),
        Just(Shape::NestedEmphasis),
        Just(Shape::NestedSuperscript),
        Just(Shape::NestedFootnoteReferences),
        Just(Shape::NestedLinkDescriptions),
        Just(Shape::NestedQuoteBlocks),
        Just(Shape::NestedList),
        Just(Shape::OpenEmphasisRun),
        Just(Shape::ClosedEmphasisRun),
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
            text.replace('\n', " ")
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

/// A line of stars in a body is nested bold to org, which emacs 30.2 reads
/// only up to its lisp depth (it fails at 5000 stars), and Holon re-parses
/// level by level; `check` writes the line into a body in every place.
#[test]
fn a_line_of_100k_markers_reads_and_writes_back() {
    for marker in ['*', '[', '('] {
        for where_ in [Place::FirstLine, Place::HeadlineBody, Place::HeadlineTitle] {
            check(Shape::Markers(marker), 100_000, where_).unwrap();
        }
    }
}

/// The fastest of three reads and writes of `line` in a headline body.
fn cost(line: &str) -> std::time::Duration {
    (0..3)
        .map(|_| {
            let start = std::time::Instant::now();
            let (document, blocks) = parse(&format!("{HEAD}{line}\n"));
            render(&document, &blocks);
            start.elapsed()
        })
        .min()
        .expect("three runs")
}

#[test]
fn a_marker_line_costs_time_linear_in_its_length() {
    let mut superlinear = Vec::new();
    for unit in [
        "*", "/", "+", "_", "[", "[[", "{", "(", "((", "~", "=", "*a* ", "*/a/* ", "[[a]] ",
    ] {
        let short = cost(&unit.repeat(1_000));
        let long = cost(&unit.repeat(4_000));
        if long > short * 8 {
            superlinear.push(format!("{unit:?}: 1k {short:?}, 4k {long:?}"));
        }
    }
    assert!(
        superlinear.is_empty(),
        "4x the length took more than 8x the time:\n{}",
        superlinear.join("\n")
    );
}

/// The fastest of three renders of the content and marks `line` reads as.
fn render_cost(line: &str) -> std::time::Duration {
    let (content, marks) = extract_inline_marks(line);
    (0..3)
        .map(|_| {
            let start = std::time::Instant::now();
            render_lossless(&content, &marks).unwrap_or_else(|e| panic!("render failed: {e:#}"));
            start.elapsed()
        })
        .min()
        .expect("three runs")
}

/// One mark every few characters is the render's worst case: a render pass
/// that compares each mark with the others is quadratic in the line. The
/// read-and-write test above uses lines too short for that to show.
#[test]
fn a_marked_line_renders_in_time_linear_in_its_length() {
    let mut superlinear = Vec::new();
    for unit in ["*a* ", "*/a/* ", "=a= ", "[[a]] "] {
        let short = render_cost(&unit.repeat(16_000 / unit.len()));
        let long = render_cost(&unit.repeat(64_000 / unit.len()));
        if long > short * 8 {
            superlinear.push(format!("{unit:?}: 16k {short:?}, 64k {long:?}"));
        }
    }
    assert!(
        superlinear.is_empty(),
        "4x the length took more than 8x the time to render:\n{}",
        superlinear.join("\n")
    );
}

/// The bytes one read and write of `line` in a headline body allocates.
fn allocated(line: &str) -> usize {
    let file = format!("{HEAD}{line}\n");
    let before = ALLOCATED.with(Cell::get);
    let (document, blocks) = parse(&file);
    render(&document, &blocks);
    ALLOCATED.with(Cell::get) - before
}

#[test]
fn a_line_of_closed_markup_allocates_linear_in_its_length() {
    let mut superlinear = Vec::new();
    for unit in ["*a* ", "*/a/* ", "=a= ", "[[a]] ", "*[[a]]* "] {
        let short = allocated(&unit.repeat(8_000 / unit.len()));
        let long = allocated(&unit.repeat(32_000 / unit.len()));
        if long > short * 6 {
            superlinear.push(format!("{unit:?}: 8k {short} B, 32k {long} B"));
        }
    }
    assert!(
        superlinear.is_empty(),
        "4x the length allocated more than 6x the bytes:\n{}",
        superlinear.join("\n")
    );
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
