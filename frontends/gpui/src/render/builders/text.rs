use super::prelude::*;
use gpui::{
    AnyElement, FontStyle, FontWeight, HighlightStyle, SharedString, StrikethroughStyle,
    StyledText, UnderlineStyle,
};
use holon_api::{InlineMark, MarkSpan, Value};
use holon_frontend::ReactiveViewModel;
use std::ops::Range;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let content = node.prop_str("content").unwrap_or_else(|| "".to_string());
    let bold = node.prop_bool("bold").unwrap_or(false);
    let size = node.prop_f64("size").unwrap_or(14.0) as f32;
    let color = node.prop_str("color").map(|s| s.to_string());

    // `marks` lives on the row data, not on props — it doesn't go through the
    // DSL args path. Fail loud on malformed JSON: stored marks must be valid.
    let marks: Option<Vec<MarkSpan>> = match node.entity().get("marks") {
        Some(Value::String(s)) | Some(Value::Json(s)) if !s.is_empty() && s != "[]" => {
            Some(holon_api::marks_from_json(s).expect("blocks.marks must be valid JSON"))
        }
        _ => None,
    };

    let mut el = div().line_height(px(26.0));
    if size > 0.0 {
        el = el.text_size(px(size));
    } else {
        el = el.text_size(px(15.0));
    }
    if content.is_empty() {
        el = el.min_w(px(1.0));
    }
    if bold {
        el = el.font_weight(FontWeight::SEMIBOLD);
    }
    if let Some(ref color_name) = color {
        el = el.text_color(resolve_color(ctx, color_name));
    }

    let inner: AnyElement = match marks {
        Some(ref m) if !m.is_empty() => {
            let highlights = build_highlights(&content, m, ctx);
            el.child(
                StyledText::new(SharedString::from(content.clone())).with_highlights(highlights),
            )
            .into_any_element()
        }
        _ => el.child(content.clone()).into_any_element(),
    };

    // When this `text(...)` resolved a `col("content")` against a live row,
    // expose the on-screen string via `BoundsRegistry` so the PBT
    // `inv-displayed-text` invariant catches non-editable text widgets that
    // diverge from `block.content_text()`. Skip tracking for static labels
    // (no row id) AND for col-bindings to other columns — `inv-displayed-text`
    // hard-compares against `block.content_text()`, so a widget reading
    // `col("name")` is *correct* but would compare wrong.
    let Some(row_id) = node.row_id() else {
        return inner;
    };
    let bound_field = node.prop_str("field");
    if bound_field.as_deref() != Some("content") {
        return inner;
    }
    let el_id = format!("text-{row_id}-content");
    let has_content = !content.is_empty();
    crate::geometry::tracked(
        el_id,
        inner,
        &ctx.bounds_registry,
        "text",
        Some(&row_id),
        has_content,
        Some(content),
    )
    .into_any_element()
}

fn resolve_color(ctx: &GpuiRenderContext, color_name: &str) -> Hsla {
    if color_name.starts_with('#') {
        let hex = color_name.trim_start_matches('#');
        if hex.len() >= 6 && hex.is_ascii() {
            let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(128);
            let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(128);
            let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(128);
            return gpui::rgba((r as u32) << 24 | (g as u32) << 16 | (b as u32) << 8 | 0xFF).into();
        }
        return tc(ctx, |t| t.foreground);
    }
    match color_name {
        "muted" | "secondary" => tc(ctx, |t| t.muted_foreground),
        "warning" => tc(ctx, |t| t.warning),
        "error" => tc(ctx, |t| t.danger),
        "success" => tc(ctx, |t| t.success),
        _ => tc(ctx, |t| t.foreground),
    }
}

/// Convert mark spans (Unicode-scalar offsets) into a sorted, non-overlapping
/// list of `(byte_range, HighlightStyle)` pairs suitable for
/// `StyledText::with_highlights`.
fn build_highlights(
    text: &str,
    marks: &[MarkSpan],
    ctx: &GpuiRenderContext,
) -> Vec<(Range<usize>, HighlightStyle)> {
    compute_segments(text, marks)
        .into_iter()
        .map(|(range, active)| (range, merge_marks(&active, ctx)))
        .collect()
}

/// Walk unique boundaries (start/end offsets) and emit one segment per run of
/// constant active marks. A mark `m` is active at position `p` iff
/// `m.start <= p < m.end`. Returns byte ranges (not char offsets) so callers
/// can hand them directly to GPUI's text APIs. Pure helper, no theme access.
fn compute_segments<'a>(
    text: &str,
    marks: &'a [MarkSpan],
) -> Vec<(Range<usize>, Vec<&'a InlineMark>)> {
    let mut char_to_byte: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    char_to_byte.push(text.len());
    let total_chars = char_to_byte.len() - 1;
    let to_byte = |c: usize| -> usize {
        assert!(
            c <= total_chars,
            "MarkSpan char offset {c} exceeds text length ({total_chars} chars)"
        );
        char_to_byte[c]
    };

    let mut boundaries: Vec<usize> = marks
        .iter()
        .flat_map(|m| [m.start, m.end])
        .filter(|&b| b <= total_chars)
        .collect();
    boundaries.sort_unstable();
    boundaries.dedup();

    let mut segments = Vec::with_capacity(boundaries.len());
    for window in boundaries.windows(2) {
        let (start_char, end_char) = (window[0], window[1]);
        let active: Vec<&InlineMark> = marks
            .iter()
            .filter(|m| m.start <= start_char && m.end >= end_char && m.start < m.end)
            .map(|m| &m.mark)
            .collect();
        if active.is_empty() {
            continue;
        }
        segments.push((to_byte(start_char)..to_byte(end_char), active));
    }
    segments
}

fn merge_marks(active: &[&InlineMark], ctx: &GpuiRenderContext) -> HighlightStyle {
    let mut style = HighlightStyle::default();
    for mark in active {
        match mark {
            InlineMark::Bold => style.font_weight = Some(FontWeight::BOLD),
            InlineMark::Italic => style.font_style = Some(FontStyle::Italic),
            InlineMark::Code => {
                style.background_color = Some(tc(ctx, |t| t.muted));
                style.color = Some(tc(ctx, |t| t.success));
            }
            InlineMark::Verbatim => {
                style.background_color = Some(tc(ctx, |t| t.muted));
            }
            InlineMark::Strike => {
                style.strikethrough = Some(StrikethroughStyle {
                    color: None,
                    thickness: px(1.0),
                });
            }
            InlineMark::Underline => {
                style.underline = Some(UnderlineStyle {
                    color: None,
                    thickness: px(1.0),
                    wavy: false,
                });
            }
            InlineMark::Link { .. } => {
                style.color = Some(tc(ctx, |t| t.accent_foreground));
                style.underline = Some(UnderlineStyle {
                    color: None,
                    thickness: px(1.0),
                    wavy: false,
                });
            }
            // Sub/Super: HighlightStyle has no baseline-shift field; renders
            // at default baseline. Phase 3+ may revisit via custom Element.
            InlineMark::Sub | InlineMark::Super => {}
        }
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_api::{EntityRef, EntityUri};

    fn span(start: usize, end: usize, mark: InlineMark) -> MarkSpan {
        MarkSpan::new(start, end, mark)
    }

    #[test]
    fn ascii_disjoint_marks_become_byte_ranges() {
        let text = "Hello, rich world!";
        let marks = vec![
            span(0, 5, InlineMark::Bold),    // "Hello"
            span(7, 11, InlineMark::Italic), // "rich"
            span(12, 17, InlineMark::Code),  // "world"
        ];
        let segments = compute_segments(text, &marks);
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].0, 0..5);
        assert_eq!(segments[1].0, 7..11);
        assert_eq!(segments[2].0, 12..17);
        assert_eq!(segments[0].1, vec![&InlineMark::Bold]);
        assert_eq!(segments[1].1, vec![&InlineMark::Italic]);
        assert_eq!(segments[2].1, vec![&InlineMark::Code]);
    }

    #[test]
    fn overlapping_marks_coalesce_into_segments() {
        let text = "abcdefgh";
        // Bold over 0..6 ("abcdef") and Italic over 2..8 ("cdefgh") —
        // expect three segments: bold-only, bold+italic, italic-only.
        let marks = vec![
            span(0, 6, InlineMark::Bold),
            span(2, 8, InlineMark::Italic),
        ];
        let segments = compute_segments(text, &marks);
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].0, 0..2);
        assert_eq!(segments[0].1, vec![&InlineMark::Bold]);
        assert_eq!(segments[1].0, 2..6);
        assert_eq!(
            segments[1].1,
            vec![&InlineMark::Bold, &InlineMark::Italic]
        );
        assert_eq!(segments[2].0, 6..8);
        assert_eq!(segments[2].1, vec![&InlineMark::Italic]);
    }

    #[test]
    fn multibyte_chars_map_to_correct_byte_offsets() {
        // "Héllo": H=1B, é=2B, l=1B, l=1B, o=1B  → 6 bytes total
        // char offsets:  H=0, é=1, l=2, l=3, o=4, end=5
        // byte offsets:  H=0, é=1, l=3, l=4, o=5, end=6
        let text = "Héllo";
        // Bold over chars 0..2 → "Hé" → bytes 0..3
        let marks = vec![span(0, 2, InlineMark::Bold)];
        let segments = compute_segments(text, &marks);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].0, 0..3, "expected byte range 0..3 for 'Hé'");
    }

    #[test]
    fn empty_marks_produces_no_segments() {
        let segments = compute_segments("hello", &[]);
        assert!(segments.is_empty());
    }

    #[test]
    fn link_mark_kept_as_single_active_entry() {
        let text = "see also";
        let marks = vec![span(
            0,
            8,
            InlineMark::Link {
                target: EntityRef::Internal {
                    id: EntityUri::block("abc"),
                },
                label: "see also".into(),
            },
        )];
        let segments = compute_segments(text, &marks);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].0, 0..8);
        assert!(matches!(segments[0].1[0], InlineMark::Link { .. }));
    }
}
