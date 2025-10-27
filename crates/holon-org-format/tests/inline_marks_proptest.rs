//! Unit-level proptest for the inline-marks round-trip.
//!
//! # Why this is NOT in `general_e2e_pbt.rs`
//!
//! Per CLAUDE.md and Phase 1.1 plan §1.3: the project's E2E PBT lives in
//! `crates/holon-integration-tests/tests/general_e2e_pbt.rs` and is the only
//! state-machine PBT that should grow. This file tests a much narrower
//! invariant — `(text, marks)` round-trip identity through the parser /
//! renderer pair — and uses proptest only as a generator for input
//! diversity, not for state-machine simulation. The plan explicitly endorses
//! unit-level proptests of this shape:
//!
//! > "**Property-based test** at the unit level: random `Vec<MarkSpan>` over
//! >  random text round-trips identically (modulo
//! >  `normalize_content_for_org_roundtrip` rules)."
//!
//! # What's tested
//!
//! `(text, marks) → render_inline_marks → extract_inline_marks → (text,
//! marks)` is identity for inputs the org parser can re-parse.
//!
//! Multibyte text is exercised via single CJK/emoji marked words separated
//! by ASCII whitespace, so scalar↔byte arithmetic in extract / render is
//! covered without tripping orgize's word-boundary requirements.
//!
//! # Known lossy cases (NOT tested here — discovered by an earlier version
//! of this proptest, documented for future fixes)
//!
//! 1. **`*X*Y` where X+Y are word-adjacent**: orgize requires the closing
//!    delimiter to be followed by whitespace / punctuation / end. So a
//!    mark on a substring that's word-adjacent to the surrounding text
//!    (e.g. Bold over chars 0..1 of "abc") renders to `*a*bc` and the
//!    parser sees that as literal text, not Bold. Real frontends apply
//!    marks to whole words / selections, so this isn't a UX issue, but
//!    it's a parser/renderer asymmetry callers should know about.
//!
//! 2. **Nested marks ending at the same position**: `*foo=bar=*` —
//!    Verbatim ending immediately before Bold's close — sometimes fails
//!    to re-parse cleanly. Rare in practice (renderer naturally avoids
//!    this when emitting from clean mark sets) but a known orgize edge
//!    case. The 27 unit tests in `inline_marks.rs::tests` cover the
//!    common nested patterns explicitly.
//!
//! # What's NOT tested (deliberate)
//!
//! - **Crossing marks**: `render_inline_marks` documents these as lossy
//!   (Org cannot represent `*A /B* C/`); a `tracing::warn!` is emitted.
//! - **Link marks**: `EntityRef` carries arbitrary URLs / block IDs;
//!   generating valid ones is a separate concern. Unit tests cover Link.
//! - **Sub / Super**: `_{…}` and `^{…}` have specific peeling logic;
//!   covered by unit tests.

use holon_api::{InlineMark, MarkSpan};
use holon_org_format::{extract_inline_marks, render_inline_marks};
use proptest::prelude::*;

/// One "word" of text — a span of safe non-whitespace chars that the
/// parser will treat as an atomic unit. Words are separated by single
/// spaces in the assembled paragraph, giving every mark boundary the
/// whitespace neighbour orgize needs to recognize a delimiter.
fn word(min_len: usize, max_len: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(safe_word_char(), min_len..=max_len)
        .prop_map(|cs| cs.into_iter().collect::<String>())
}

fn safe_word_char() -> impl Strategy<Value = char> {
    // ASCII alphanumerics + a CJK/emoji char so scalar↔byte conversion
    // is regularly exercised. No org-markup punctuation.
    prop_oneof![
        7 => any::<usize>().prop_map(|i| {
            const CS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
            CS.chars().nth(i % CS.chars().count()).unwrap()
        }),
        3 => any::<usize>().prop_map(|i| {
            const MB: &[char] = &['你', '好', '世', '界', 'こ', 'ん', 'α', 'β', '🌍', '🦀'];
            MB[i % MB.len()]
        }),
    ]
}

/// Six commonly-used mark variants whose delimiters are single chars.
/// Sub/Super (`_{…}`, `^{…}`) and Link (carries data) are excluded —
/// covered by the unit tests in `inline_marks.rs`.
fn arbitrary_mark() -> impl Strategy<Value = InlineMark> {
    prop_oneof![
        Just(InlineMark::Bold),
        Just(InlineMark::Italic),
        Just(InlineMark::Code),
        Just(InlineMark::Verbatim),
        Just(InlineMark::Strike),
        Just(InlineMark::Underline),
    ]
}

/// Build a paragraph as `[word][ word]…` and a `Vec<MarkSpan>` where each
/// generated mark covers exactly one word. This guarantees:
///   - Marks are non-crossing (single-word ranges, separated by spaces).
///   - Every mark boundary is at whitespace / start / end (parser-friendly).
///   - Multiple words can carry different marks; some words can be unmarked.
fn paragraph_with_marks() -> impl Strategy<Value = (String, Vec<MarkSpan>)> {
    // 1..=6 words; each word has 0 or 1 mark.
    prop::collection::vec(
        (word(1, 6), prop::option::weighted(0.6, arbitrary_mark())),
        1..=6,
    )
    .prop_map(|words_and_marks| {
        let mut text = String::new();
        let mut marks = Vec::new();
        for (i, (word, maybe_mark)) in words_and_marks.into_iter().enumerate() {
            if i > 0 {
                text.push(' ');
            }
            let start = text.chars().count();
            text.push_str(&word);
            let end = text.chars().count();
            if let Some(mark) = maybe_mark {
                marks.push(MarkSpan::new(start, end, mark));
            }
        }
        (text, marks)
    })
}

fn marks_equivalent(a: &[MarkSpan], b: &[MarkSpan]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut a_sorted: Vec<&MarkSpan> = a.iter().collect();
    let mut b_sorted: Vec<&MarkSpan> = b.iter().collect();
    let key = |m: &&MarkSpan| (m.start, m.end, format!("{:?}", m.mark));
    a_sorted.sort_by_key(key);
    b_sorted.sort_by_key(key);
    a_sorted == b_sorted
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        ..ProptestConfig::default()
    })]

    /// Whole-word marks separated by whitespace round-trip identically
    /// through render → extract. This is the load-bearing invariant: every
    /// Loro outbound reconcile and every org file save relies on it for
    /// any mark a real editor would produce (selections snap to word/char
    /// boundaries, not into the middle of a word adjacent to other text).
    #[test]
    fn whole_word_marks_round_trip((text, marks) in paragraph_with_marks()) {
        let rendered = render_inline_marks(&text, &marks);
        let (back_text, back_marks) = extract_inline_marks(&rendered);

        prop_assert_eq!(&back_text, &text, "text changed across round-trip");
        prop_assert!(
            marks_equivalent(&marks, &back_marks),
            "marks differ after round-trip:\n  text:     {:?}\n  rendered: {:?}\n  in:       {:?}\n  out:      {:?}",
            text, rendered, marks, back_marks
        );
    }
}
