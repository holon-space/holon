//! Convert `[MarkSpan]` into a fully-partitioned `Vec<gpui::TextRun>`
//! suitable for `WindowTextSystem::shape_text`.
//!
//! # Why a different output than Phase 2's text builder
//!
//! Phase 2's `frontends/gpui/src/render/builders/text.rs` produces
//! `Vec<(Range<usize>, HighlightStyle)>` — *delta* highlights against an
//! ambient `TextStyle`, fed to `gpui::StyledText::with_highlights`. That's
//! ideal for read-only rendering inside a `Div`.
//!
//! The editor uses a different paint path: `WindowTextSystem::shape_text`
//! takes `&[TextRun]` where each run carries a fully-resolved style
//! (font + color + underline + strikethrough + bg) and a byte length, and
//! the runs concatenate to cover every byte of the text. That gives the
//! editor `WrappedLine::position_for_index` and `closest_index_for_position`
//! for caret placement and mouse hit-testing, which `StyledText` doesn't
//! expose. (See Phase 0.2 spike at `frontends/gpui/examples/rich_input_spike.rs`
//! for the proof-of-concept paint pass this output feeds.)
//!
//! # Algorithm
//!
//! 1. Collect unique scalar boundaries from mark starts/ends, plus 0 and
//!    `text.chars().count()`.
//! 2. Walk consecutive boundary pairs; for each segment, compute the active
//!    marks and merge them into a `TextRun`.
//! 3. Convert scalar offsets to byte offsets at the segment level so the
//!    `len: usize` field on `TextRun` is bytes (per gpui's contract).

use std::ops::Range;

use gpui::{px, FontStyle, FontWeight, Hsla, StrikethroughStyle, TextRun, UnderlineStyle};
use holon_api::{InlineMark, MarkSpan};

/// Theme-resolved colors and base typography for the editor's paint pass.
///
/// Pre-resolving theme lookups means `marks_to_text_runs` is pure (no
/// `GpuiRenderContext` parameter) and unit-testable without a window.
/// The editor view fills this in once per render pass.
#[derive(Clone, Debug)]
pub struct RichTextStyle {
    pub default_font: gpui::Font,
    pub default_color: Hsla,
    /// Background for `Code` and `Verbatim` runs.
    pub muted_bg: Hsla,
    /// Foreground override for `Code` runs.
    pub code_color: Hsla,
    /// Foreground override for `Link` runs.
    pub link_color: Hsla,
}

/// Produce a fully-partitioned `Vec<TextRun>` covering every byte of `text`.
///
/// Empty `text` returns an empty vector. Runs always sum to `text.len()`
/// bytes; gpui's `shape_text` will panic if this invariant is violated.
pub fn marks_to_text_runs(text: &str, marks: &[MarkSpan], style: &RichTextStyle) -> Vec<TextRun> {
    if text.is_empty() {
        return Vec::new();
    }

    // Char-index → byte-index lookup, inclusive of the past-the-end position.
    let mut char_to_byte: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    char_to_byte.push(text.len());
    let total_chars = char_to_byte.len() - 1;

    // Boundaries in char offsets — start, end, plus every mark edge.
    let mut boundaries: Vec<usize> = Vec::with_capacity(marks.len() * 2 + 2);
    boundaries.push(0);
    boundaries.push(total_chars);
    for m in marks {
        if m.start <= total_chars {
            boundaries.push(m.start);
        }
        if m.end <= total_chars {
            boundaries.push(m.end);
        }
    }
    boundaries.sort_unstable();
    boundaries.dedup();

    let mut runs = Vec::with_capacity(boundaries.len());
    for window in boundaries.windows(2) {
        let (start_char, end_char) = (window[0], window[1]);
        if start_char == end_char {
            continue;
        }
        let active: Vec<&InlineMark> = marks
            .iter()
            .filter(|m| m.start <= start_char && m.end >= end_char && m.start < m.end)
            .map(|m| &m.mark)
            .collect();
        let byte_len = char_to_byte[end_char] - char_to_byte[start_char];
        runs.push(build_run(byte_len, &active, style));
    }
    runs
}

/// Build a single `TextRun` for a segment given its active marks and style.
fn build_run(byte_len: usize, active: &[&InlineMark], style: &RichTextStyle) -> TextRun {
    let mut font = style.default_font.clone();
    let mut color = style.default_color;
    let mut background_color: Option<Hsla> = None;
    let mut underline: Option<UnderlineStyle> = None;
    let mut strikethrough: Option<StrikethroughStyle> = None;

    for mark in active {
        match mark {
            InlineMark::Bold => font.weight = FontWeight::BOLD,
            InlineMark::Italic => font.style = FontStyle::Italic,
            InlineMark::Code => {
                background_color = Some(style.muted_bg);
                color = style.code_color;
            }
            InlineMark::Verbatim => {
                background_color = Some(style.muted_bg);
            }
            InlineMark::Strike => {
                strikethrough = Some(StrikethroughStyle {
                    color: None,
                    thickness: px(1.0),
                });
            }
            InlineMark::Underline => {
                underline = Some(UnderlineStyle {
                    color: None,
                    thickness: px(1.0),
                    wavy: false,
                });
            }
            InlineMark::Link { .. } => {
                color = style.link_color;
                underline = Some(UnderlineStyle {
                    color: None,
                    thickness: px(1.0),
                    wavy: false,
                });
            }
            // Sub/Super not supported via TextRun (no baseline-shift field).
            InlineMark::Sub | InlineMark::Super => {}
        }
    }

    TextRun {
        len: byte_len,
        font,
        color,
        background_color,
        underline,
        strikethrough,
    }
}

/// Convert a Unicode-scalar range to a byte range for `text`.
///
/// Asserts the range fits within `text`. The editor uses this to translate
/// `RichTextSelection` (scalar offsets) into byte-based hit-test coordinates
/// for `WrappedLine::position_for_index`.
pub fn scalar_range_to_bytes(text: &str, range: Range<usize>) -> Range<usize> {
    let mut char_to_byte: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    char_to_byte.push(text.len());
    let total = char_to_byte.len() - 1;
    assert!(
        range.start <= total && range.end <= total,
        "scalar_range_to_bytes: {range:?} exceeds text length {total} chars"
    );
    char_to_byte[range.start]..char_to_byte[range.end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::FontWeight;

    fn test_style() -> RichTextStyle {
        RichTextStyle {
            default_font: gpui::font(".SystemUIFont"),
            default_color: Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.9,
                a: 1.0,
            },
            muted_bg: Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.2,
                a: 1.0,
            },
            code_color: Hsla {
                h: 0.4,
                s: 0.5,
                l: 0.7,
                a: 1.0,
            },
            link_color: Hsla {
                h: 0.6,
                s: 0.7,
                l: 0.6,
                a: 1.0,
            },
        }
    }

    #[test]
    fn empty_text_yields_empty_runs() {
        let runs = marks_to_text_runs("", &[], &test_style());
        assert!(runs.is_empty());
    }

    #[test]
    fn unmarked_text_yields_single_default_run() {
        let runs = marks_to_text_runs("hello", &[], &test_style());
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].len, 5);
        assert_eq!(runs[0].font.weight, FontWeight::default());
        assert_eq!(runs[0].font.style, FontStyle::Normal);
        assert!(runs[0].background_color.is_none());
        assert!(runs[0].underline.is_none());
    }

    #[test]
    fn runs_partition_text_completely() {
        // Bold over [0..5) of "hello world" — expect TWO runs covering all
        // 11 bytes: bold "hello", default " world".
        let runs = marks_to_text_runs(
            "hello world",
            &[MarkSpan::new(0, 5, InlineMark::Bold)],
            &test_style(),
        );
        assert_eq!(runs.len(), 2);
        let total_bytes: usize = runs.iter().map(|r| r.len).sum();
        assert_eq!(total_bytes, "hello world".len());
        assert_eq!(runs[0].font.weight, FontWeight::BOLD);
        assert_eq!(runs[1].font.weight, FontWeight::default());
    }

    #[test]
    fn overlapping_marks_merge_in_segment() {
        // "abcdefgh" — Bold[0..6), Italic[2..8) → 3 runs:
        //   [0..2) bold-only, [2..6) bold+italic, [6..8) italic-only.
        let runs = marks_to_text_runs(
            "abcdefgh",
            &[
                MarkSpan::new(0, 6, InlineMark::Bold),
                MarkSpan::new(2, 8, InlineMark::Italic),
            ],
            &test_style(),
        );
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].len, 2);
        assert_eq!(runs[0].font.weight, FontWeight::BOLD);
        assert_eq!(runs[0].font.style, FontStyle::Normal);
        assert_eq!(runs[1].len, 4);
        assert_eq!(runs[1].font.weight, FontWeight::BOLD);
        assert_eq!(runs[1].font.style, FontStyle::Italic);
        assert_eq!(runs[2].len, 2);
        assert_eq!(runs[2].font.weight, FontWeight::default());
        assert_eq!(runs[2].font.style, FontStyle::Italic);
    }

    #[test]
    fn multibyte_run_lengths_are_bytes() {
        // "Héllo": H=1B, é=2B, l=1B, l=1B, o=1B → 6 bytes total
        // Bold over chars [0..2) covers "Hé" → 3 bytes.
        let runs = marks_to_text_runs(
            "Héllo",
            &[MarkSpan::new(0, 2, InlineMark::Bold)],
            &test_style(),
        );
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].len, 3, "Hé spans 3 bytes");
        assert_eq!(runs[1].len, 3, "llo spans 3 bytes");
        assert_eq!(runs[0].font.weight, FontWeight::BOLD);
    }

    #[test]
    fn link_mark_applies_color_and_underline() {
        let runs = marks_to_text_runs(
            "click here",
            &[MarkSpan::new(
                0,
                5,
                InlineMark::Link {
                    target: holon_api::EntityRef::External {
                        url: "https://example.com".into(),
                    },
                    label: "click".into(),
                },
            )],
            &test_style(),
        );
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].len, 5);
        assert!(runs[0].underline.is_some());
        assert_eq!(runs[0].color, test_style().link_color);
        // The trailing " here" is unmarked — no underline.
        assert!(runs[1].underline.is_none());
        assert_eq!(runs[1].color, test_style().default_color);
    }

    #[test]
    fn code_mark_applies_bg_and_color() {
        let runs = marls_with_code();
        assert_eq!(runs.len(), 3);
        let code = &runs[1];
        assert_eq!(code.background_color, Some(test_style().muted_bg));
        assert_eq!(code.color, test_style().code_color);
    }

    fn marls_with_code() -> Vec<TextRun> {
        marks_to_text_runs(
            "foo bar baz",
            &[MarkSpan::new(4, 7, InlineMark::Code)],
            &test_style(),
        )
    }

    #[test]
    fn scalar_range_to_bytes_handles_multibyte() {
        // "a你好b" — char offsets: a=0, 你=1, 好=2, b=3, end=4
        // byte offsets:           a=0, 你=1, 好=4, b=7, end=8
        // Range [1..3) chars → bytes [1..7).
        let r = scalar_range_to_bytes("a你好b", 1..3);
        assert_eq!(r, 1..7);
    }

    #[test]
    fn scalar_range_to_bytes_full_range() {
        let r = scalar_range_to_bytes("hello", 0..5);
        assert_eq!(r, 0..5);
    }
}
