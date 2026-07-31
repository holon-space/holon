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
