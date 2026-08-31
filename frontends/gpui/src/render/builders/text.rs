use std::ops::Range;

use gpui::AnyElement;
use gpui::FontStyle;
use gpui::FontWeight;
use gpui::HighlightStyle;
use gpui::SharedString;
use gpui::StrikethroughStyle;
use gpui::StyledText;
use gpui::UnderlineStyle;
use holon_api::EntityRef;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::link_parser::LinkTargetClassifier;
use holon_frontend::ReactiveViewModel;

use super::prelude::*;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let content = node.prop_str("content").unwrap_or_default();
    let mut bold = node.prop_bool("bold").unwrap_or(false);
    let mut size = node.prop_f64("size").unwrap_or(14.0) as f32;
    let color = node.prop_str("color").map(|s| s.to_string());

    // Disclosed placeholder: a genuinely empty col-bound value must never
    // render as a blank row (fail-loud, never fake). `text(col(...),
    // #{empty: "(untitled)"})` supplies the disclosed stand-in, shown muted +
    // italic so a degraded value reads as degraded. Display-only: `content`
    // stays the real (empty) value below, so mark spans and inv-displayed-text
    // tracking still compare against the truth, not the placeholder.
    let empty_placeholder = node.prop_str("empty");
    let (display, is_placeholder) =
        holon_api::render_eval::text_display(&content, empty_placeholder.as_deref());
    let display = display.to_string();

    // A semantic `style` keyword (e.g. `#{style: "h1"}` on the page-title
    // variant) resolves to the heading type scale here at render time — the
    // single place both the initial build and the fast-path props recompute
    // funnel through, so the size can never regress to body size on a CDC
    // recompute. The semantic style wins over the numeric `size:` default.
    if let Some(style) = node.prop_str("style") {
        if let Some(scale) = holon_api::render_eval::text_style_font_size(&style) {
            size = scale;
            bold = bold || style.starts_with('h');
        }
    }

    // `marks` lives on the row data, not on props — it doesn't go through the
    // DSL args path. Read through the shared projection boundary so this
    // builder heals and discloses a corrupt `(content, marks)` pair exactly as
    // `rendered_text` does, instead of keeping a second, stricter copy of the
    // same read.
    let marks: Option<Vec<MarkSpan>> =
        Some(holon_frontend::link_segments::marks_of(&node.entity())).filter(|m| !m.is_empty());

    let mut el = div().line_height(px(26.0));
    if size > 0.0 {
        el = el.text_size(px(size));
    } else {
        el = el.text_size(px(15.0));
    }
    if content.is_empty() {
        el = el.min_w(px(1.0));
    }
    // A fixed box for a glyph that has to line up down a column. Glyph advances
    // differ (`⚠` is wider than `●`), so a content-sized box would put each
    // row's symbol at its own x. Set by widgets that EMIT text nodes as a
    // column cell (`integration_status`); a plain `text(...)` leaves it unset
    // and sizes to its content as before.
    if let Some(box_width) = node.prop_f64("width") {
        el = el.w(px(box_width as f32)).flex_shrink_0().text_center();
    } else if node.prop_str("field").is_some_and(|f| f != "content") {
        // A LABEL read from a data column (a sidebar row's name) must yield
        // when its row runs out of width. A flex item's automatic minimum is
        // its min-content width, so without this a long label pushes whatever
        // follows it — the status glyph — past the row's edge instead of
        // clipping itself. Scoped to column-bound labels: a block's own
        // `content` text keeps the width behaviour the editor expects.
        //
        // `truncate()` (clip + nowrap + …) rather than a bare clip: a name cut
        // off mid-glyph with no mark reads as the whole name, so the ellipsis
        // is what makes the shortening visible instead of silent.
        el = el.min_w(px(0.0)).truncate();
    }
    if bold {
        el = el.font_weight(FontWeight::SEMIBOLD);
    }
    if let Some(ref color_name) = color {
        el = el.text_color(resolve_color(ctx, color_name));
    }
    if is_placeholder {
        el = el.text_color(tc(ctx, |t| t.muted_foreground)).italic();
    }

    let mut styled_runs: Option<Vec<holon_api::StyledRun>> = None;
    let inner: AnyElement = match marks {
        Some(ref m) if !m.is_empty() => {
            let highlights = build_highlights(&content, m, ctx);
            styled_runs = Some(observed_styled_runs(&highlights));
            el.child(
                StyledText::new(SharedString::from(content.clone())).with_highlights(highlights),
            )
            .into_any_element()
        }
        _ => el.child(display.clone()).into_any_element(),
    };

    // When this `text(...)` resolved a `col(...)` against a live row, expose the
    // on-screen string via `BoundsRegistry` so the PBT `inv-displayed-text`
    // invariant catches non-editable text widgets that diverge from
    // `block.content_text()`. Static labels (no row id) have nothing to bind to.
    let Some(row_id) = node.row_id() else {
        return inner;
    };
    let Some(field) = node.prop_str("field") else {
        return inner;
    };
    // That invariant compares against the BLOCK's content, so a block row's
    // non-`content` binding (`text(col("name"))`) is right on screen yet wrong
    // under that oracle, and stays untracked. A row of another entity — an
    // `integration:` mirror row — has no block content to be compared against,
    // so what it paints is left readable for the rungs that read it.
    if field != "content" && row_id.starts_with("block:") {
        return inner;
    }
    let el_id = format!("text-{row_id}-{field}");
    let has_content = !content.is_empty();
    let mut tracker = crate::geometry::tracked(
        el_id,
        inner,
        &ctx.bounds_registry,
        "text",
        Some(&row_id),
        has_content,
        Some(std::sync::Arc::from(content)),
    );
    if let Some(runs) = styled_runs {
        tracker = tracker.with_styled_runs(runs);
    }
    tracker.into_any_element()
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
/// `StyledText::with_highlights`. Shared with the `rendered_text` read-mode
/// builder so both read paths style marks identically.
pub(crate) fn build_highlights(
    text: &str,
    marks: &[MarkSpan],
    ctx: &GpuiRenderContext,
) -> Vec<(Range<usize>, HighlightStyle)> {
    compute_segments(text, marks)
        .into_iter()
        .map(|(range, active)| (range, merge_marks(&active, ctx)))
        .collect()
}

/// Extract the theme-independent paint fingerprint from the highlight runs the
/// widget actually hands to `StyledText::with_highlights`. This reads the REAL
/// painted `HighlightStyle` (not a recomputation from marks), so a renderer
/// that silently dropped a mark's weight/decoration shows up here as plain
/// flags. `inv-paint-text-styling` compares this against
/// `holon_api::style_fingerprint`.
pub(crate) fn observed_styled_runs(
    highlights: &[(Range<usize>, HighlightStyle)],
) -> Vec<holon_api::StyledRun> {
    highlights
        .iter()
        .map(|(r, style)| holon_api::StyledRun {
            start: r.start,
            end: r.end,
            flags: observed_style_flags(style),
        })
        .collect()
}

/// Map a painted `HighlightStyle` to its theme-independent paint attributes.
fn observed_style_flags(h: &HighlightStyle) -> holon_api::StyleFlags {
    holon_api::StyleFlags {
        bold: h.font_weight == Some(FontWeight::BOLD),
        italic: h.font_style == Some(FontStyle::Italic),
        underline: h.underline.is_some(),
        strikethrough: h.strikethrough.is_some(),
        background: h.background_color.is_some(),
    }
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

/// How a link run is painted.
///
/// Split out from theme lookup so the RULE is unit-testable without a window:
/// the paint oracle's `StyleFlags` carries neither colour nor waviness, so this
/// function is the only place the disclosure can be asserted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkDecoration {
    /// Followable: accent colour, straight underline.
    Healthy,
    /// Resolves to nothing and cannot be followed — muted colour + wavy
    /// underline, so it never wears the healthy-link accent.
    Unresolved,
}

/// The mark says only that the target is scheme-shaped; whether that scheme
/// names a live entity is a property of the registry, so it is asked here, per
/// paint.
pub(crate) fn link_decoration(
    target: &EntityRef,
    classifier: &LinkTargetClassifier,
) -> LinkDecoration {
    match target {
        // A colon-bearing target that names no entity at all (a wiki path like
        // `Meeting/Notes:2026`) is not an entity link and is not unresolved —
        // it is an ordinary page target, painted like any other.
        EntityRef::Scheme { .. } => match target.entity_uri() {
            Some(uri) if classifier.resolves_entity(&uri) => LinkDecoration::Healthy,
            Some(_) => LinkDecoration::Unresolved,
            None => LinkDecoration::Healthy,
        },
        EntityRef::External { .. } | EntityRef::Name { .. } => LinkDecoration::Healthy,
    }
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
            InlineMark::Link { target, .. } => {
                let (color, wavy) = match link_decoration(target, ctx.services.link_classifier()) {
                    LinkDecoration::Healthy => (tc(ctx, |t| t.accent_foreground), false),
                    LinkDecoration::Unresolved => (tc(ctx, |t| t.muted_foreground), true),
                };
                style.color = Some(color);
                style.underline = Some(UnderlineStyle {
                    color: None,
                    thickness: px(1.0),
                    wavy,
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
    use holon_api::EntityUri;

    use super::*;

    fn span(start: usize, end: usize, mark: InlineMark) -> MarkSpan {
        MarkSpan::new(start, end, mark)
    }

    /// An unregistered scheme must be DISCLOSED, not merely typed: it resolves
    /// to nothing and cannot be followed, so painting it with the healthy-link
    /// accent would tell the reader it works.
    #[test]
    fn an_unknown_scheme_link_is_disclosed_as_unresolved() {
        assert_eq!(
            link_decoration(
                &EntityRef::from_uri(&EntityUri::parse("cc-session:abc").expect("valid uri")),
                &LinkTargetClassifier::default(),
            ),
            LinkDecoration::Unresolved,
            "an unknown-scheme link must not be painted as a healthy link"
        );
    }

    /// The other three targets are all followable, so none may pick up the
    /// degraded treatment.
    #[test]
    fn resolvable_link_targets_stay_healthy() {
        for target in [
            EntityRef::from_uri(&EntityUri::parse("block:abc").expect("valid uri")),
            EntityRef::External {
                url: "https://example.com".to_string(),
            },
            EntityRef::Name {
                name: "Some Page".to_string(),
            },
        ] {
            assert_eq!(
                link_decoration(&target, &LinkTargetClassifier::default()),
                LinkDecoration::Healthy,
                "{target:?} is followable and must keep the healthy-link treatment"
            );
        }
    }

    /// The pair below is the whole point of live classification: the SAME
    /// stored mark decorates differently depending on the registry it is read
    /// against, so a link ingested before its provider connects heals on its
    /// own instead of staying broken forever.
    #[test]
    fn a_scheme_link_is_unresolved_while_its_scheme_is_unregistered() {
        assert_eq!(
            link_decoration(&t_widget_link(), &LinkTargetClassifier::default()),
            LinkDecoration::Unresolved,
        );
    }

    #[test]
    fn the_same_scheme_link_is_healthy_once_the_scheme_registers() {
        assert_eq!(
            link_decoration(
                &t_widget_link(),
                &LinkTargetClassifier::with_schemes(["t-widget"]),
            ),
            LinkDecoration::Healthy,
        );
    }

    fn t_widget_link() -> EntityRef {
        EntityRef::from_uri(&EntityUri::parse("t-widget:abc").expect("valid uri"))
    }

    /// A colon in a LATER path segment does not make a target scheme-shaped, so
    /// `entity_uri()` returns `None` and there is no registration question to
    /// ask: it is an ordinary page target and paints Healthy. Painting it
    /// Unresolved would tell the user a perfectly good page link is broken.
    ///
    /// Registration cannot change this — asserted against a classifier that
    /// DOES know `cc-session`, so the answer is a property of the target's
    /// shape, not of the registry.
    ///
    /// Intended-for-now gap: unlike a `Name` target, clicking one of these is
    /// inert (no lazy page creation) — see `rendered_text.rs`.
    #[test]
    fn a_colon_bearing_page_path_is_healthy_not_unresolved() {
        for raw in ["Meeting/Notes:2026", "Areas/cc-session:abc"] {
            let target = EntityRef::Scheme {
                raw: raw.to_string(),
            };
            assert_eq!(
                link_decoration(&target, &LinkTargetClassifier::default()),
                LinkDecoration::Healthy,
                "{raw} names no entity, so it must not be painted as a broken link"
            );
            assert_eq!(
                link_decoration(&target, &LinkTargetClassifier::with_schemes(["cc-session"])),
                LinkDecoration::Healthy,
                "{raw} must not become an entity link just because a scheme registered"
            );
        }
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
        let marks = vec![span(0, 6, InlineMark::Bold), span(2, 8, InlineMark::Italic)];
        let segments = compute_segments(text, &marks);
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].0, 0..2);
        assert_eq!(segments[0].1, vec![&InlineMark::Bold]);
        assert_eq!(segments[1].0, 2..6);
        assert_eq!(segments[1].1, vec![&InlineMark::Bold, &InlineMark::Italic]);
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
                target: EntityRef::from_uri(&EntityUri::block("abc")),
                label: "see also".into(),
            },
        )];
        let segments = compute_segments(text, &marks);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].0, 0..8);
        assert!(matches!(segments[0].1[0], InlineMark::Link { .. }));
    }
}
