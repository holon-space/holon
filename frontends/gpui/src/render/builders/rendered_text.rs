use std::ops::Range;

use gpui::MouseButton;
use gpui::MouseDownEvent;
use gpui::SharedString;
use gpui::StyledText;
use holon_api::EntityRef;
use holon_api::EntityUri;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::Value;
use holon_api::marks_from_json;
use holon_frontend::operations::OperationIntent;

use super::prelude::*;
use crate::render::builders::text::build_highlights;
use crate::render::rich_text_runs::scalar_range_to_bytes;

/// Read-only sibling of `editable_text`. Renders the block's content as
/// static text. A click calls `services.set_focus` (ADR 0010: focus is pure
/// in-memory state), which flips the `is_focused` variant in
/// `block_profile.yaml` and swaps in `editable_text` on the next render. The
/// freshly-mounted editor grabs window focus off the `focused_block` signal.
///
/// When the block's marks contain `InlineMark::Link` variants, the content is
/// split into text and link segments. Link segments are clickable and dispatch
/// `navigation.focus` to navigate the main panel to the target block.
pub fn render(node: &holon_frontend::ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let content = node.prop_str("content").unwrap_or_default();
    let field = node
        .prop_str("field")
        .unwrap_or_else(|| "content".to_string());

    let Some(row_id) = node.row_id() else {
        return static_inner(&content, ctx).into_any_element();
    };

    let el_id = format!("rendered-text-{row_id}-{field}");
    let has_content = !content.is_empty();
    let services = ctx.services.clone();

    // Parsed once for the click target.
    // ALLOW(entity_uri_from_raw): render-spec rendered_text node row_id (boundary)
    let block_uri = EntityUri::from_raw(&row_id);

    // Extract marks (same pattern as text.rs). Fail loud on malformed JSON.
    let marks: Option<Vec<MarkSpan>> = match node.entity().get("marks") {
        Some(Value::String(s)) | Some(Value::Json(s)) if !s.is_empty() && s != "[]" => {
            Some(marks_from_json(s).expect("blocks.marks must be valid JSON"))
        }
        _ => None,
    };

    // The read-mode styling fingerprint the widget ACTUALLY paints: only the
    // styled branch produces highlight runs (derived from the same
    // `build_highlights` fed to `StyledText`), so a marked block that falls to
    // the plain branch leaves this `None` — exactly the read-mode styling-drop
    // bug `inv-paint-text-styling` catches on the painted output.
    let mut styled_runs: Option<Vec<holon_api::StyledRun>> = None;
    let inner: AnyElement = if !has_content {
        // Empty block: placeholder text, plain click-to-focus. There is no
        // meaningful caret position, so the caret defaults to end-of-text
        // (offset 0) on mount.
        click_to_focus(
            &el_id,
            static_inner(&content, ctx).into_any_element(),
            block_uri,
            services,
        )
        .into_any_element()
    } else if wants_styled_render(has_content, marks.as_deref()) {
        // One `build_highlights` per paint (as on main) — `styled_run_render`
        // returns the painted styled-run fingerprint from that SAME computation,
        // so the observation adds only a cheap run extraction, never a second
        // partition pass (which would double the per-frame cost and widen
        // windowed settle races).
        let (el, runs) =
            styled_run_render(&el_id, &content, marks.as_ref().unwrap(), &block_uri, services, ctx);
        styled_runs = Some(runs);
        el
    } else {
        // Content, no marks: a click both focuses the block AND seeds the caret
        // at the clicked offset (identity styled→buffer map). This closes the
        // long-standing "click in the middle of a block, caret lands elsewhere"
        // dogfood bug for plain blocks.
        plain_caret_click(&el_id, &content, block_uri, services)
    };

    let mut tracker = crate::geometry::tracked(
        el_id,
        inner,
        &ctx.bounds_registry,
        "rendered_text",
        Some(&row_id),
        has_content,
        Some(std::sync::Arc::from(content)),
    );
    if let Some(runs) = styled_runs {
        tracker = tracker.with_styled_runs(runs);
    }
    tracker.into_any_element()
}

/// Read mode renders styled runs whenever the block has content AND any mark of
/// any kind. The bug this replaced (dogfood 2026-07-22 bug 1) gated styling on
/// a *Link* mark only, so a block whose marks were Bold/Italic/Underline fell
/// to `static_inner` (plain text) and its formatting silently vanished — even
/// though the editor styled the same marks. Gate on presence of any mark.
fn wants_styled_render(has_content: bool, marks: Option<&[MarkSpan]>) -> bool {
    has_content && marks.is_some_and(|m| !m.is_empty())
}

/// Map a byte offset in the read-projection (styled) text to the byte offset in
/// the editor buffer that mounts for this block.
///
/// Identity today: the read projection renders the STRIPPED content verbatim
/// (marks re-style existing chars; they add none), so styled text == buffer
/// content byte-for-byte. The caret seed is a byte offset — split/join arm it
/// via `navigation::placement_to_offset`, and the editor mount consumes it
/// through `InputState`'s `offset_to_position` — so this stays in the byte
/// domain.
///
/// Raw-edit increment I2 replaces this ONE function's body with a lookup into
/// the `RawOffsetMap` produced by `holon-org-format`'s
/// `render_inline_marks_with_map`, once the editor buffer holds RAW org text
/// and label chars diverge from raw chars. No call site changes — the seam is
/// deliberately this single function.
fn styled_offset_to_buffer_offset(styled_byte_offset: usize) -> usize {
    styled_byte_offset
}

/// Plain (mark-less) content whose click focuses the block AND seeds the caret
/// at the clicked offset.
///
/// Renders the content as a `StyledText` (no highlights — visually identical to
/// `static_inner`'s metrics) so we can clone its `TextLayout` and hit-test the
/// mouse-down position. A click inside the glyphs arms
/// `set_focus_with_caret(block, offset)`; a click past the glyphs (where
/// `TextLayout::index_for_position` returns `Err`) falls back to plain
/// `set_focus` — a disclosed degradation (caret defaults to end-of-text), never
/// a fabricated offset.
fn plain_caret_click(
    el_id: &str,
    content: &str,
    block: EntityUri,
    services: std::sync::Arc<dyn holon_frontend::reactive::BuilderServices>,
) -> AnyElement {
    let styled = StyledText::new(SharedString::from(content.to_string()));
    // Clone the layout handle (shared `Rc`) BEFORE the element is consumed, so
    // the `on_mouse_down` closure can hit-test against the painted layout.
    let layout = styled.layout().clone();
    div()
        .id(hashed_id(el_id))
        .w_full()
        .px(px(12.0))
        .py(px(8.0))
        .text_sm()
        .line_height(gpui::Rems(1.25))
        .cursor_pointer()
        .child(styled)
        .on_mouse_down(
            MouseButton::Left,
            move |ev: &MouseDownEvent, _window, _cx| match layout.index_for_position(ev.position) {
                Ok(byte_offset) => services.set_focus_with_caret(
                    block.clone(),
                    styled_offset_to_buffer_offset(byte_offset),
                ),
                Err(_) => services.set_focus(Some(block.clone())),
            },
        )
        .into_any_element()
}

/// Render content with marks as a single wrapping styled-text flow.
///
/// One `StyledText` carries every mark kind (bold/italic/underline/strike/code
/// and link color, via the shared `text::build_highlights`), so inline links no
/// longer split the block into separate `flex_row` children that stack onto
/// their own lines (dogfood F3). A single `on_mouse_down` on the text-bearing
/// div hit-tests the click against the text's own `TextLayout`: a click over a
/// link span navigates (/ follows a dangling link), any other click seeds the
/// caret at the clicked offset to enter edit mode.
///
/// `InteractiveText` is deliberately NOT used here. Its click hitbox paints on
/// top of a wrapping div, so a parent `on_mouse_down`'s `is_hovered` guard
/// never fires — the caret capture would silently no-op. Owning the click on
/// the text div itself (same mechanism as the plain `plain_caret_click` path)
/// avoids that shadowing and keeps one code path for both.
fn styled_run_render(
    el_id: &str,
    content: &str,
    marks: &[MarkSpan],
    block_uri: &EntityUri,
    services: std::sync::Arc<dyn holon_frontend::reactive::BuilderServices>,
    ctx: &GpuiRenderContext,
) -> (AnyElement, Vec<holon_api::StyledRun>) {
    // Never abort on a corrupt persisted mark. A mark span outliving its
    // content ("N..M exceeds text length K") aborts EVERY paint of this block —
    // a page permanently un-openable. Disclosed degraded render: log loud with
    // the block id (fail-loud, not silent) and let `scalar_range_to_bytes`
    // clamp the offending span. The durable fix is the read-boundary
    // `canonicalize_marks_against`; this is the render-layer safety net.
    let content_chars = content.chars().count();
    for ms in marks {
        if ms.end > content_chars {
            // First occurrence per block stays loud (ERROR) so the corruption
            // is disclosed once; per-frame repaint repeats drop to debug! to
            // avoid the 1847×/block spam (BugFunnel N7). No global mute — a
            // different corrupt block gets its own loud first line.
            let first = crate::render::warn_throttle::first_time_seen((
                "rendered_text_mark_overflow",
                block_uri.as_str(),
            ));
            if first {
                tracing::error!(
                    block_id = %block_uri.as_str(),
                    mark_start = ms.start,
                    mark_end = ms.end,
                    content_chars,
                    "rendered_text: mark span exceeds content length; rendering \
                     degraded (clamped). Corrupt persisted marks for this block."
                );
            } else {
                tracing::debug!(
                    block_id = %block_uri.as_str(),
                    mark_start = ms.start,
                    mark_end = ms.end,
                    content_chars,
                    "rendered_text: mark span exceeds content length (repeat; \
                     first occurrence for this block logged at ERROR)."
                );
            }
        }
    }

    // One flowing, wrapping styled text: all marks become highlight runs.
    let highlights = build_highlights(content, marks, ctx);
    // The paint-observable fingerprint, extracted from the SAME highlight runs
    // handed to `StyledText` below (no second partition pass). Read back by
    // `inv-paint-text-styling`.
    let styled_runs = crate::render::builders::text::observed_styled_runs(&highlights);
    let styled =
        StyledText::new(SharedString::from(content.to_string())).with_highlights(highlights);
    // Clone the layout handle (shared `Rc`) BEFORE `styled` is consumed by the
    // div, so the `on_mouse_down` closure can hit-test the click position
    // against the painted layout.
    let layout = styled.layout().clone();

    // Segments carry each link span's byte range + target, so a hit-tested byte
    // offset resolves to either "link (navigate)" or "text (seed caret)".
    let segments = build_content_segments(content, marks);

    let block = block_uri.clone();
    let element = div()
        .id(hashed_id(el_id))
        .w_full()
        .px(px(12.0))
        .py(px(8.0))
        .text_sm()
        .line_height(gpui::Rems(1.25))
        .cursor_pointer()
        .child(styled)
        .on_mouse_down(
            MouseButton::Left,
            move |ev: &MouseDownEvent, _window, _cx| {
                let Ok(byte_offset) = layout.index_for_position(ev.position) else {
                    // Click past the glyphs: plain focus, caret defaults to
                    // end-of-text (disclosed degradation, never a fabricated
                    // offset).
                    services.set_focus(Some(block.clone()));
                    return;
                };
                match link_at_offset(&segments, byte_offset) {
                    Some(EntityRef::Internal { id }) => {
                        services.dispatch_intent(nav_focus(id.to_string()));
                    }
                    Some(EntityRef::External { url }) => {
                        services.dispatch_intent(nav_focus(url.clone()));
                    }
                    Some(EntityRef::Name { name }) => {
                        // Dangling link: create the page chain for this name (lazy
                        // page-create, 2026-07-10 links ruling) and navigate the
                        // main region to the new leaf; the next render re-resolves
                        // the healed junction and this segment becomes `Internal`.
                        services.follow_dangling_link(name.clone(), "main".to_string());
                    }
                    None => services.set_focus_with_caret(
                        block.clone(),
                        styled_offset_to_buffer_offset(byte_offset),
                    ),
                }
            },
        )
        .into_any_element();
    (element, styled_runs)
}

/// Build a `navigation.focus` intent for the main region targeting `block_id`.
fn nav_focus(block_id: String) -> OperationIntent {
    OperationIntent::new(
        "navigation".into(),
        "focus".into(),
        [
            ("region".into(), Value::String("main".into())),
            ("block_id".into(), Value::String(block_id)),
        ]
        .into_iter()
        .collect(),
    )
}

/// One segment of content: either plain text or a link.
#[derive(Debug, PartialEq, Clone)]
struct ContentSegment {
    text: String,
    byte_range: Range<usize>,
    is_link: bool,
    link_target: Option<EntityRef>,
}

/// Split `content` at link boundaries into alternating text/link segments.
/// Non-overlapping; links are sorted by start position.
fn build_content_segments(content: &str, marks: &[MarkSpan]) -> Vec<ContentSegment> {
    let mut link_spans: Vec<(Range<usize>, &EntityRef)> = marks
        .iter()
        .filter_map(|ms| match &ms.mark {
            InlineMark::Link { target, .. } => {
                Some((scalar_range_to_bytes(content, ms.start..ms.end), target))
            }
            _ => None,
        })
        .collect();

    if link_spans.is_empty() {
        let len = content.len();
        return vec![ContentSegment {
            text: content.to_string(),
            byte_range: 0..len,
            is_link: false,
            link_target: None,
        }];
    }

    link_spans.sort_by_key(|(r, _)| (r.start, r.end));

    let mut segments = Vec::new();
    let mut pos = 0;

    for (range, target) in &link_spans {
        if range.start > pos {
            segments.push(ContentSegment {
                text: content[pos..range.start].to_string(),
                byte_range: pos..range.start,
                is_link: false,
                link_target: None,
            });
        }
        segments.push(ContentSegment {
            text: content[range.clone()].to_string(),
            byte_range: range.clone(),
            is_link: true,
            link_target: Some((*target).clone()),
        });
        pos = range.end;
    }

    if pos < content.len() {
        segments.push(ContentSegment {
            text: content[pos..].to_string(),
            byte_range: pos..content.len(),
            is_link: false,
            link_target: None,
        });
    }

    segments
}

/// Find the link target at `byte_offset` within segments.
fn link_at_offset<'a>(segments: &'a [ContentSegment], byte_offset: usize) -> Option<&'a EntityRef> {
    segments
        .iter()
        .find(|s| s.is_link && s.byte_range.contains(&byte_offset))
        .and_then(|s| s.link_target.as_ref())
}

/// Static text element matching `editable_text`'s visual metrics so the
/// transition from read-only → editable doesn't cause a perceptible jump
/// when focus changes.
///
/// `Input::render` (gpui-component) — even with `appearance(false)` —
/// always applies `input_px(self.size)`, `input_py(self.size)`,
/// `input_text_size(self.size)`, and `line_height(Rems(1.25))`. For the
/// default `Size::Medium` that is `px(12)` horizontal, `px(8)` vertical,
/// `text_sm`, and `line_height` 1.25rem. We mirror those exactly here so
/// the swap from `rendered_text` → `editable_text` doesn't shift x/y or
/// resize the glyphs.
fn static_inner(content: &str, _: &GpuiRenderContext) -> Div {
    let display: String = if content.is_empty() {
        "Type here".to_string()
    } else {
        content.to_string()
    };
    let mut el = div()
        .w_full()
        .px(px(12.0))
        .py(px(8.0))
        .text_sm()
        .line_height(gpui::Rems(1.25))
        .child(display);
    if content.is_empty() {
        el = el.text_color(gpui::Hsla {
            h: 0.0,
            s: 0.0,
            l: 0.5,
            a: 0.5,
        });
    }
    el
}

#[cfg(test)]
mod tests {
    use holon_api::EntityUri;

    use super::*;

    fn mark_link(start: usize, end: usize, target: EntityRef) -> MarkSpan {
        MarkSpan::new(
            start,
            end,
            InlineMark::Link {
                target,
                label: String::new(),
            },
        )
    }

    fn internal_uri(id: &str) -> EntityRef {
        EntityRef::Internal {
            id: EntityUri::parse(id).unwrap(),
        }
    }

    #[test]
    fn build_content_segments_no_links_returns_single_text_segment() {
        let content = "Hello world";
        let segments = build_content_segments(content, &[]);
        assert_eq!(segments.len(), 1);
        assert!(!segments[0].is_link);
        assert_eq!(segments[0].text, "Hello world");
        assert!(segments[0].link_target.is_none());
    }

    #[test]
    fn build_content_segments_with_link_splits_correctly() {
        let content = "before [[link]] after";
        let link_text = "[[link]]";
        let link_start = content.find(link_text).unwrap();
        let link_end = link_start + "[[link]]".len();
        let link_target = internal_uri("block:abc-123");

        let marks = vec![mark_link(link_start, link_end, link_target)];

        let segments = build_content_segments(content, &marks);
        assert_eq!(segments.len(), 3);

        assert!(!segments[0].is_link);
        assert_eq!(segments[0].text, "before [[link]] after"[..link_start]);

        assert!(segments[1].is_link);
        assert_eq!(segments[1].text, link_text);
        assert!(segments[1].link_target.is_some());

        assert!(!segments[2].is_link);
        assert_eq!(segments[2].text, " after");
    }

    #[test]
    fn build_content_segments_with_multiple_links() {
        let content = "A [[one]] B [[two]] C";
        let marks = vec![
            mark_link(
                content.find("[[one]]").unwrap(),
                content.find("[[one]]").unwrap() + "[[one]]".len(),
                internal_uri("block:one"),
            ),
            mark_link(
                content.find("[[two]]").unwrap(),
                content.find("[[two]]").unwrap() + "[[two]]".len(),
                internal_uri("block:two"),
            ),
        ];

        let segments = build_content_segments(content, &marks);
        assert_eq!(segments.len(), 5);

        assert!(!segments[0].is_link);
        assert_eq!(segments[0].text, "A ");

        assert!(segments[1].is_link);
        assert_eq!(segments[1].text, "[[one]]");

        assert!(!segments[2].is_link);
        assert_eq!(segments[2].text, " B ");

        assert!(segments[3].is_link);
        assert_eq!(segments[3].text, "[[two]]");

        assert!(!segments[4].is_link);
        assert_eq!(segments[4].text, " C");
    }

    #[test]
    fn link_at_offset_finds_link() {
        let content = "before [[link]] after";
        let link_text = "[[link]]";
        let link_start = content.find(link_text).unwrap();
        let link_end = link_start + link_text.len();
        let link_target = internal_uri("block:abc-123");
        let marks = vec![mark_link(link_start, link_end, link_target.clone())];
        let segments = build_content_segments(content, &marks);

        let found = link_at_offset(&segments, link_start);
        assert!(found.is_some());
        assert_eq!(*found.unwrap(), link_target);
    }

    #[test]
    fn link_at_offset_outside_link_returns_none() {
        let content = "before [[link]] after";
        let link_text = "[[link]]";
        let link_start = content.find(link_text).unwrap();
        let link_end = link_start + link_text.len();
        let link_target = internal_uri("block:abc-123");
        let marks = vec![mark_link(link_start, link_end, link_target)];
        let segments = build_content_segments(content, &marks);

        assert!(link_at_offset(&segments, 0).is_none());
        assert!(link_at_offset(&segments, link_end).is_none());
    }

    #[test]
    fn build_content_segments_handles_link_at_start() {
        let content = "[[link]] after";
        let marks = vec![mark_link(
            0,
            "[[link]]".len(),
            internal_uri("block:abc-123"),
        )];
        let segments = build_content_segments(content, &marks);

        assert_eq!(segments.len(), 2);
        assert!(segments[0].is_link);
        assert_eq!(segments[0].text, "[[link]]");
        assert!(!segments[1].is_link);
        assert_eq!(segments[1].text, " after");
    }

    #[test]
    fn build_content_segments_handles_link_at_end() {
        let content = "before [[link]]";
        let link_start = content.find("[[link]]").unwrap();
        let link_end = content.len();
        let marks = vec![mark_link(
            link_start,
            link_end,
            internal_uri("block:abc-123"),
        )];
        let segments = build_content_segments(content, &marks);

        assert_eq!(segments.len(), 2);
        assert!(!segments[0].is_link);
        assert_eq!(segments[0].text, "before ");
        assert!(segments[1].is_link);
        assert_eq!(segments[1].text, "[[link]]");
    }

    #[test]
    fn build_content_segments_multibyte_content_splits_correctly() {
        // "\u{00FC}" = 'ü' = 2 bytes (0xC3 0xBC).
        // "a" (byte 0) + "b" (byte 1) + "ü" (bytes 2-3) + "c" (byte 4)
        // = 4 scalar offsets (0..4), 5 bytes total.
        let content = "ab\u{00FC}c";
        let marks = vec![mark_link(2, 3, internal_uri("block:abc-123"))];
        let segments = build_content_segments(content, &marks);

        assert_eq!(segments.len(), 3);
        assert!(!segments[0].is_link);
        assert_eq!(segments[0].text, "ab");
        assert_eq!(segments[0].byte_range, 0..2);
        assert!(segments[1].is_link);
        assert_eq!(segments[1].text, "\u{00FC}");
        assert_eq!(segments[1].byte_range, 2..4);
        assert!(!segments[2].is_link);
        assert_eq!(segments[2].text, "c");
    }

    // --- dogfood 2026-07-22 bug 1 (marks-on-blur) + F3 (inline-link stacking) ---

    fn mark(start: usize, end: usize, m: InlineMark) -> MarkSpan {
        MarkSpan::new(start, end, m)
    }

    /// Bug 1: a block whose ONLY marks are non-link (bold/underline) MUST
    /// render styled. The old gate keyed on a Link mark, so this returned
    /// false and the block fell to plain `static_inner` — the exact escape.
    #[test]
    fn bold_only_marks_want_styled_render() {
        let marks = vec![mark(0, 4, InlineMark::Bold)];
        assert!(
            wants_styled_render(true, Some(&marks)),
            "non-link marks must route through the styled renderer"
        );
    }

    #[test]
    fn underline_only_marks_want_styled_render() {
        let marks = vec![mark(0, 4, InlineMark::Underline)];
        assert!(wants_styled_render(true, Some(&marks)));
    }

    #[test]
    fn link_marks_still_want_styled_render() {
        let marks = vec![mark_link(0, 4, internal_uri("block:x"))];
        assert!(wants_styled_render(true, Some(&marks)));
    }

    #[test]
    fn no_marks_stays_plain() {
        assert!(!wants_styled_render(true, None));
        assert!(!wants_styled_render(true, Some(&[])));
    }

    #[test]
    fn empty_content_never_styled_even_with_marks() {
        let marks = vec![mark(0, 4, InlineMark::Bold)];
        assert!(!wants_styled_render(false, Some(&marks)));
    }

    fn rich_style() -> crate::render::rich_text_runs::RichTextStyle {
        crate::render::rich_text_runs::RichTextStyle {
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

    /// Bug 1, styled-run oracle on Martin's exact repro. The stored content is
    /// the stripped text; marks are Bold over "content" (14..21) and Underline
    /// over "block" (25..30). The read-mode styling pipeline the fix now routes
    /// through must yield a BOLD run and an underlined run — not one plain run.
    #[test]
    fn martins_repro_yields_bold_and_underline_runs() {
        use crate::render::rich_text_runs::marks_to_text_runs;
        let content = "Formatting of content in block content";
        assert_eq!(&content[14..21], "content");
        assert_eq!(&content[25..30], "block");
        let marks = vec![
            mark(14, 21, InlineMark::Bold),
            mark(25, 30, InlineMark::Underline),
        ];
        let runs = marks_to_text_runs(content, &marks, &rich_style());

        // More than one run => the text was partitioned by marks, not left plain.
        assert!(runs.len() > 1, "styled content must produce multiple runs");
        assert!(
            runs.iter().any(|r| r.font.weight == gpui::FontWeight::BOLD),
            "a bold run must exist over \"content\""
        );
        assert!(
            runs.iter().any(|r| r.underline.is_some()),
            "an underlined run must exist over \"block\""
        );
        // Runs cover every byte (single flowing text, not dropped spans).
        let total: usize = runs.iter().map(|r| r.len).sum();
        assert_eq!(total, content.len());
    }

    /// F3: an inline link mid-sentence must partition into ordered segments
    /// that tile the whole content contiguously (no gaps, no overlap). This
    /// is the data a single wrapping `StyledText`/`InteractiveText`
    /// consumes, replacing the old per-segment `flex_row` children that
    /// stacked onto separate lines.
    #[test]
    fn inline_link_partition_tiles_content_contiguously() {
        let content = "See the Target Page reference inline in this sentence";
        let start = content.find("Target Page").unwrap();
        let end = start + "Target Page".len();
        let marks = vec![mark_link(start, end, internal_uri("block:target-page-doc"))];
        let segments = build_content_segments(content, &marks);

        // Contiguous cover 0..len, in order, no gaps/overlap.
        assert_eq!(segments.first().unwrap().byte_range.start, 0);
        assert_eq!(segments.last().unwrap().byte_range.end, content.len());
        for w in segments.windows(2) {
            assert_eq!(
                w[0].byte_range.end, w[1].byte_range.start,
                "segments must tile"
            );
        }
        // Exactly one link segment, carrying its nav target (interactivity kept).
        let links: Vec<_> = segments.iter().filter(|s| s.is_link).collect();
        assert_eq!(links.len(), 1);
        assert!(
            links[0].link_target.is_some(),
            "link segment keeps its target"
        );
        assert_eq!(links[0].text, "Target Page");
    }

    /// Link-interactivity data: the clickable ranges handed to
    /// `InteractiveText` line up 1:1 with the segment partition, and each
    /// link range maps to its target by index (the `on_click(ix)` lookup).
    /// A click on a non-link range maps to `None` (focus the block).
    #[test]
    fn clickable_ranges_align_with_targets_by_index() {
        let content = "a [[one]] b [[two]] c";
        let one_s = content.find("[[one]]").unwrap();
        let two_s = content.find("[[two]]").unwrap();
        let marks = vec![
            mark_link(one_s, one_s + "[[one]]".len(), internal_uri("block:one")),
            mark_link(two_s, two_s + "[[two]]".len(), internal_uri("block:two")),
        ];
        let segments = build_content_segments(content, &marks);
        let ranges: Vec<_> = segments.iter().map(|s| s.byte_range.clone()).collect();
        let targets: Vec<_> = segments.iter().map(|s| s.link_target.clone()).collect();

        assert_eq!(ranges.len(), targets.len());
        // Indices of link segments carry Internal targets; text segments are None.
        for (seg, tgt) in segments.iter().zip(&targets) {
            assert_eq!(seg.is_link, tgt.is_some());
        }
        assert_eq!(targets.iter().filter(|t| t.is_some()).count(), 2);
    }
}
