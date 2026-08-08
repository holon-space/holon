//! The store↔disk cycle for MARKED blocks must reach a fixed point, and reach
//! it without losing or inventing content bytes.
//!
//! One cycle is `render_block_content` → `extract_inline_marks` — the PROD
//! emit path, degradation ladder included.
//! A single cycle proves nothing here: the render half MINTS marks (a quoted
//! literal comes back carrying `Verbatim`), so the state a block occupies on
//! cycle 2 is one this code produced, not one any generator wrote. Every rung
//! below therefore runs at least TWO cycles and asserts both the emitted bytes
//! and the stored `(content, marks)` state stop changing.

use holon_api::EntityRef;
use holon_api::EntityUri;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::block::Block;
use holon_block_roundtrip_testing::marked_content_strategy;
use holon_org_format::RenderFidelity;
use holon_org_format::expected_reparse;
use holon_org_format::extract_inline_marks;
use holon_org_format::render_block_content_checked;
use proptest::prelude::*;

/// Render → parse → render → parse, through the PROD emit path
/// (`render_block_content`) rather than the pure quoting function — the
/// degradation ladder is part of what has to reach a fixed point. Returns the
/// settled `(bytes, content)`; panics with the full trace when a cycle moves.
fn assert_fixed_point(content: &str, marks: &[MarkSpan], cycles: usize) -> (String, String) {
    assert_fixed_point_against(content, marks, cycles, None)
}

/// `expected_first_cycle` is an INDEPENDENT oracle — the generator's own view
/// of what one cycle must produce. When it is `None` the crate's contract
/// function stands in, which is fine for hand-written cases whose answer is
/// obvious, but useless as a regression net: an oracle that asks the code under
/// test what "correct" means goes wrong in lockstep with it.
fn assert_fixed_point_against(
    content: &str,
    marks: &[MarkSpan],
    cycles: usize,
    expected_first_cycle: Option<&str>,
) -> (String, String) {
    let mut state = (content.to_string(), marks.to_vec());
    let mut history: Vec<(String, String, Vec<MarkSpan>)> = Vec::new();
    for cycle in 0..cycles {
        let mut block = Block::new_text(
            EntityUri::block("fixed-point"),
            EntityUri::block("parent"),
            &state.0,
        );
        block.marks = Some(state.1.clone());
        let (emitted, fidelity) = render_block_content_checked(&block);
        let (next_content, next_marks) = extract_inline_marks(&emitted);
        // The contract, not a restatement of it: content may differ from the
        // stored bytes ONLY by link adoption, and only where no protective or
        // data-bearing mark has already sealed the span.
        //
        // `ContentUnpreserved` is the one exemption, and it is not a loophole:
        // it means every rung of the ladder REFUSED, i.e. org genuinely cannot
        // express this state (a data-bearing mark strictly inside a markup
        // literal, say). Reaching it is loudly disclosed in prod; silently
        // reaching it here would be the bug, so the fidelity must say so.
        if fidelity != RenderFidelity::ContentUnpreserved {
            let oracle = match (cycle, expected_first_cycle) {
                (0, Some(independent)) => independent.to_string(),
                _ => expected_reparse(&state.0, &state.1),
            };
            assert_eq!(
                next_content, oracle,
                "cycle {cycle}: content changed in a way the contract does not allow (fidelity \
                 {fidelity:?}).\nemitted {emitted:?}\nhistory: {history:#?}"
            );
        }
        history.push((emitted, next_content.clone(), next_marks.clone()));
        state = (next_content, next_marks);
    }
    // Settlement is required on the FIRST cycle, not eventually. Bytes that
    // keep moving are the echo-loop condition, and "it converges by cycle 3"
    // still means three write-backs and three re-ingests of the same block.
    assert_eq!(
        history[0].0, history[1].0,
        "emitted bytes did not settle immediately.\nhistory: {history:#?}"
    );
    // Two consecutive cycles must agree on BOTH the bytes and the stored marks.
    let last = history.last().expect("at least one cycle");
    let prev = &history[history.len() - 2];
    assert_eq!(
        prev.0, last.0,
        "emitted bytes never settled.\nhistory: {history:#?}"
    );
    assert_eq!(
        prev.2, last.2,
        "stored marks never settled.\nhistory: {history:#?}"
    );
    (last.0.clone(), last.1.clone())
}

fn span(start: usize, end: usize, mark: InlineMark) -> MarkSpan {
    MarkSpan { start, end, mark }
}

fn link(start: usize, end: usize) -> MarkSpan {
    MarkSpan {
        start,
        end,
        mark: InlineMark::Link {
            target: EntityRef::External {
                url: "https://example.com".to_string(),
            },
            label: String::new(),
        },
    }
}

/// EVERY emission must settle on its FIRST cycle, whatever rung produced it.
///
/// Bytes that re-render differently are the echo-loop condition: write-back
/// rewrites the file, the watcher re-ingests, and the pair never agrees. That
/// is a worse failure than losing a mark, so it binds even on the rungs that
/// have already given up on preserving content — "we could not represent this"
/// is never a licence to emit churn.
fn assert_settles_immediately(content: &str, marks: &[MarkSpan]) -> String {
    let mut block = Block::new_text(
        EntityUri::block("settle"),
        EntityUri::block("parent"),
        content,
    );
    block.marks = Some(marks.to_vec());
    let (first, fidelity) = render_block_content_checked(&block);

    let (c2, m2) = extract_inline_marks(&first);
    let mut next = Block::new_text(EntityUri::block("settle"), EntityUri::block("parent"), &c2);
    next.marks = Some(m2.clone());
    let (second, _) = render_block_content_checked(&next);

    assert_eq!(
        first, second,
        "emission did not settle on the first cycle (fidelity {fidelity:?}).\ncontent \
         {content:?}\nmarks {marks:?}\nre-parsed as {c2:?} + {m2:?}"
    );
    first
}

/// A `Link` mark whose span CROSSES a markup literal's boundary. The parser
/// cannot mint this — but Peritext can: "make link" over a selection that
/// starts mid-identifier.
///
/// Org has no way to express it (the link delimiters would have to split the
/// literal), so the emission necessarily degrades. What it must NOT do is
/// degrade into bytes that re-render differently again.
#[test]
fn link_crossing_a_markup_literal_settles_immediately() {
    assert_settles_immediately("/a/ _a_", &[link(2, 7)]);
    assert_settles_immediately("_a_ _a_", &[link(2, 7)]);
    assert_settles_immediately("the __init__ method", &[link(6, 14)]);
}

/// A `Link` mark over content that CONTAINS raw link syntax. Also
/// unrepresentable — nested `[[…]]` is not a thing — and also required to
/// settle rather than churn.
#[test]
fn link_over_raw_link_syntax_settles_immediately() {
    assert_settles_immediately("see [[a][b]] here", &[link(0, 17)]);
    assert_settles_immediately("[[a][b]]", &[link(0, 8)]);
}

/// The shape the fix itself manufactures: a user bolds an identifier, the
/// render half quotes the identifier, and the re-ingest hands back Bold and
/// Verbatim CO-EXTENSIVE over the same span. Emitting those two non-LIFO
/// (`*=__init__*=`) is not valid org, so the next parse swallows the
/// delimiters into the content — worse than the original bug.
#[test]
fn coextensive_marks_reach_a_fixed_point() {
    assert_fixed_point("__init__", &[span(0, 8, InlineMark::Bold)], 3);
}

/// Same nesting question with two ORDINARY user marks and no quoting
/// involved: co-extensive Bold+Italic must nest, not interleave.
#[test]
fn coextensive_user_marks_nest() {
    let emitted = assert_fixed_point(
        "hello",
        &[span(0, 5, InlineMark::Bold), span(0, 5, InlineMark::Italic)],
        3,
    )
    .0;
    assert!(
        emitted == "*/hello/*" || emitted == "/*hello*/",
        "co-extensive marks must nest LIFO, got {emitted:?}"
    );
}

#[test]
fn mark_flush_against_a_markup_literal_reaches_a_fixed_point() {
    // Bold ends exactly where the literal begins.
    assert_fixed_point("bold __init__", &[span(0, 4, InlineMark::Bold)], 3);
    // Bold starts exactly where the literal ends.
    assert_fixed_point("__init__ bold", &[span(9, 13, InlineMark::Bold)], 3);
}

#[test]
fn mark_containing_a_markup_literal_with_slack_reaches_a_fixed_point() {
    assert_fixed_point("a __init__ b", &[span(0, 12, InlineMark::Bold)], 3);
}

#[test]
fn verbatim_mark_over_a_markup_literal_reaches_a_fixed_point() {
    assert_fixed_point("__init__", &[span(0, 8, InlineMark::Verbatim)], 3);
}

#[test]
fn unmarked_markup_literal_reaches_a_fixed_point() {
    assert_fixed_point("the __default__ profile", &[], 3);
}

/// Raw `[[…]]` syntax sitting in UNMARKED content — what every non-org write
/// path stores when a user types a link (bulk create, MCP, the editor cell).
///
/// Adoption normalizes the target and the label: `[[a  ]]` adopts to the
/// content `a`, so the padded bytes are already spent by the time the file is
/// re-read. The emission must therefore be the adopted form, which settles on
/// the first cycle and costs nothing adoption was not taking anyway. Emitting
/// the padded bytes instead makes the file disagree with its own re-parse.
#[test]
fn a_raw_link_with_a_padded_target_settles_immediately() {
    assert_eq!(assert_settles_immediately("[[wJZ9  ]]", &[]), "[[wJZ9]]");
}

#[test]
fn a_raw_link_with_a_padded_label_settles_immediately() {
    assert_eq!(assert_settles_immediately("[[a][b ]]", &[]), "[[a][b]]");
}

/// Padding is the ONLY thing given up: the rung stays `Exact`, so no caller
/// downstream of `RenderFidelity` is told the block lost content, and nothing
/// is disclosed. Without this the fix could be satisfied by settling on a
/// degraded rung.
#[test]
fn a_raw_link_with_a_padded_target_keeps_the_exact_rung() {
    let mut block = Block::new_text(
        EntityUri::block("padded-link"),
        EntityUri::block("parent"),
        "see [[wJZ9  ]] here",
    );
    block.marks = Some(Vec::new());
    let (emitted, fidelity) = render_block_content_checked(&block);

    assert_eq!(emitted, "see [[wJZ9]] here");
    assert_eq!(fidelity, RenderFidelity::Exact);
    assert_eq!(extract_inline_marks(&emitted).0, "see wJZ9 here");
}

/// A link that is ALREADY canonical must not be rewritten — the normalization
/// is a repair, not a pass every link takes.
#[test]
fn a_canonical_raw_link_is_emitted_byte_for_byte() {
    assert_eq!(
        assert_settles_immediately("see [[a][b]] here", &[]),
        "see [[a][b]] here"
    );
}

/// A link that adopts to NOTHING is the one shape normalization must refuse.
///
/// Rewriting `[[   ]]` to what adoption leaves would emit the empty string:
/// settled, `Exact`, silent — and the user's bytes gone with no disclosure.
/// Settlement is not worth buying at that price, so these keep the raw bytes
/// and stay on the loud rung. `ContentUnpreserved` is the assertion, not a
/// tolerated leftover: the emission must SAY it could not represent this.
#[test]
fn a_link_that_adopts_to_nothing_keeps_its_bytes_and_stays_loud() {
    for content in ["[[   ]]", "[[]]", "[[a][ ]]", "[[  ][  ]]", "a [[  ]] b"] {
        let mut block = Block::new_text(
            EntityUri::block("empty-adoption"),
            EntityUri::block("parent"),
            content,
        );
        block.marks = Some(Vec::new());
        let (emitted, fidelity) = render_block_content_checked(&block);

        assert_eq!(
            emitted, content,
            "{content:?}: the bytes must survive rather than be silently erased"
        );
        assert_eq!(
            fidelity,
            RenderFidelity::ContentUnpreserved,
            "{content:?}: refusing to normalize must stay disclosed, not pass as Exact"
        );
    }
}

/// The 2026-08-07 dogfood payload, pinned as behaviour rather than as a bug.
///
/// A `Code` mark on `b` between two LITERAL `~` characters is unrepresentable
/// in org: the code delimiter IS `~`, so the only bytes that could carry both
/// meanings are `~~b~~`, which org reads as ONE code span over `~b~`. The
/// emission therefore drops the mark, re-seals the literal as `=~b~=`, keeps
/// every content byte, and settles on the first cycle.
///
/// What this locks is the RUNG. `ProtectiveDropped` says the content survived;
/// `ContentUnpreserved` would say the ladder fell through to pass 2 and the
/// bytes on disk no longer re-parse to what the store holds. The sighting was
/// repeatedly attributed to the terminal "no emission settles" branch, which
/// this proves is not the branch taken.
#[test]
fn a_code_mark_between_literal_code_delimiters_keeps_every_byte_and_settles() {
    let content = "nested a ~b~ c";
    let marks = [span(10, 11, InlineMark::Code)];
    let mut block = Block::new_text(
        EntityUri::block("dogfood-2026-08-07"),
        EntityUri::block("parent"),
        content,
    );
    block.marks = Some(marks.to_vec());
    let (emitted, fidelity) = render_block_content_checked(&block);

    assert_eq!(fidelity, RenderFidelity::ProtectiveDropped);
    assert_eq!(emitted, "nested a =~b~= c");
    assert_eq!(
        extract_inline_marks(&emitted).0,
        content,
        "the content bytes must survive the dropped mark; emitted {emitted:?}"
    );
    assert_fixed_point(content, &marks, 3);
}

/// The same span with a STYLING mark instead. Same impossibility, one rung
/// higher: nothing protective was given up, so the ladder must stop at
/// `StylingDropped` rather than walk further down.
#[test]
fn a_styling_mark_between_literal_code_delimiters_stops_at_the_styling_rung() {
    let content = "nested a ~b~ c";
    let marks = [span(10, 11, InlineMark::Bold)];
    let mut block = Block::new_text(
        EntityUri::block("dogfood-2026-08-07-styling"),
        EntityUri::block("parent"),
        content,
    );
    block.marks = Some(marks.to_vec());
    let (emitted, fidelity) = render_block_content_checked(&block);

    assert_eq!(fidelity, RenderFidelity::StylingDropped);
    assert_eq!(emitted, "nested a =~b~= c");
    assert_eq!(extract_inline_marks(&emitted).0, content);
    assert_fixed_point(content, &marks, 3);
}

/// The discriminating control, and the reason the two rows above are an org
/// LIMITATION and not a renderer defect: a literal `~b~` in the content costs
/// nothing as long as the mark does not live inside it, and having BOTH quote
/// delimiters in the content costs nothing either. If any of these ever
/// degrades, the ladder has become over-eager and the rows above stop
/// describing org rather than the code.
#[test]
fn a_mark_outside_the_literal_keeps_full_fidelity() {
    for (content, mark) in [
        ("lit ~x~ and b", span(12, 13, InlineMark::Code)),
        ("has = and ~ and b", span(16, 17, InlineMark::Code)),
        // A mark spanning the literal delimiters themselves IS representable —
        // `~~b~~` is exactly one code span over `~b~`.
        ("nested a ~b~ c", span(9, 12, InlineMark::Code)),
    ] {
        let mut block = Block::new_text(
            EntityUri::block("control"),
            EntityUri::block("parent"),
            content,
        );
        block.marks = Some(vec![mark.clone()]);
        let (emitted, fidelity) = render_block_content_checked(&block);
        assert_eq!(
            fidelity,
            RenderFidelity::Exact,
            "{content:?} + {mark:?} must render exactly; emitted {emitted:?}"
        );
        assert_fixed_point(content, &[mark], 3);
    }
}

/// The class, not the instances. Any `(content, marks)` state ANY producer
/// can mint must be render-safe — see `MarkedContent` for why generating marks
/// by parsing org text cannot reach most of those states.
proptest! {
    #![proptest_config(ProptestConfig {
        cases: 600,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn any_generated_store_state_reaches_a_fixed_point(state in marked_content_strategy()) {
        assert_fixed_point_against(
            &state.content,
            &state.marks,
            3,
            Some(&state.expected_after_cycle),
        );
    }
}
