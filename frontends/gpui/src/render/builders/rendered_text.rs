use gpui::MouseButton;
use gpui::MouseDownEvent;
use gpui::SharedString;
use gpui::StyledText;
use holon_api::EntityUri;
use holon_api::MarkSpan;
use holon_frontend::link_segments::LinkClickAction;
use holon_frontend::link_segments::link_at_offset;
use holon_frontend::link_segments::link_click_action;
use holon_frontend::link_segments::link_content_segments;
use holon_frontend::link_segments::marks_of;
use holon_frontend::link_segments::nav_focus;
use holon_frontend::link_segments::wants_styled_render;

use super::prelude::*;
use crate::render::builders::text::build_highlights;

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

    let marks = marks_of(&node.entity());

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
    } else if wants_styled_render(&content, &marks) {
        // One `build_highlights` per paint (as on main) — `styled_run_render`
        // returns the painted styled-run fingerprint from that SAME computation,
        // so the observation adds only a cheap run extraction, never a second
        // partition pass (which would double the per-frame cost and widen
        // windowed settle races).
        let (el, runs) = styled_run_render(&el_id, &content, &marks, &block_uri, services, ctx);
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
/// Bring every span inside `0..=content_chars` so the downstream consumers —
/// `build_highlights`, and the shared `link_content_segments` whose contract is
/// to ASSERT on an out-of-range mark — only ever see in-range input.
///
/// `.max(start)` is the anti-inversion half and is not redundant:
/// `MarkSpan::new` asserts `start <= end`, but the derived `Deserialize`
/// bypasses that assert and `canonicalize_marks` only drops zero-width spans,
/// so a persisted `{"start":5,"end":2}` reaches here. Without the clamp it
/// would abort the very paint this whole path exists to keep alive.
fn clamp_marks_to(content_chars: usize, marks: &[MarkSpan]) -> Vec<MarkSpan> {
    marks
        .iter()
        .map(|ms| {
            let start = ms.start.min(content_chars);
            let end = ms.end.min(content_chars).max(start);
            MarkSpan::new(start, end, ms.mark.clone())
        })
        .collect()
}

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
    // the block id (fail-loud, not silent) and clamp the offending span here,
    // so everything downstream — `build_highlights` and the shared
    // `link_content_segments`, whose own contract is to assert on an
    // out-of-range mark — only ever sees in-range spans. The durable fix is the
    // read-boundary `canonicalize_marks_against`; this is the render-layer net.
    let content_chars = content.chars().count();
    let clamped = clamp_marks_to(content_chars, marks);
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
    let segments = link_content_segments(content, &clamped);

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
            move |ev: &MouseDownEvent, _window, cx| {
                let Ok(byte_offset) = layout.index_for_position(ev.position) else {
                    // Click past the glyphs: plain focus, caret defaults to
                    // end-of-text (disclosed degradation, never a fabricated
                    // offset).
                    services.set_focus(Some(block.clone()));
                    return;
                };
                let action = link_click_action(
                    link_at_offset(&segments, byte_offset),
                    services.link_classifier(),
                );
                match action {
                    LinkClickAction::OpenUrl(url) => cx.open_url(&url),
                    LinkClickAction::Navigate(uri) => services.dispatch_intent(nav_focus(uri)),
                    LinkClickAction::FollowDangling(name) => {
                        services.follow_dangling_link(name, "main".to_string());
                    }
                    LinkClickAction::SeedCaret => services.set_focus_with_caret(
                        block.clone(),
                        styled_offset_to_buffer_offset(byte_offset),
                    ),
                }
            },
        )
        .into_any_element();
    (element, styled_runs)
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
    use holon_api::InlineMark;

    use super::*;

    fn mark(start: usize, end: usize, m: InlineMark) -> MarkSpan {
        MarkSpan::new(start, end, m)
    }

    /// A mark reaching past its own content must be clamped, not fatal — the
    /// alternative is a block that aborts every paint, i.e. a permanently
    /// un-openable page.
    #[test]
    fn clamp_brings_an_overlong_span_inside_the_content() {
        let out = clamp_marks_to(4, &[mark(1, 99, InlineMark::Bold)]);
        assert_eq!((out[0].start, out[0].end), (1, 4));
    }

    /// The inversion half. `MarkSpan::new` asserts `start <= end`, so if the
    /// clamp ever loses its `.max(start)` this test aborts rather than fails —
    /// which is exactly the production failure it guards.
    #[test]
    fn clamp_repairs_a_persisted_inverted_span() {
        // Round-tripped rather than hand-written so the fixture tracks the
        // real wire format, and so the test itself proves the constructor's
        // `start <= end` assert is bypassable by deserialization.
        let mut wire = serde_json::to_value(mark(2, 5, InlineMark::Bold)).expect("serialize");
        wire["start"] = 5.into();
        wire["end"] = 2.into();
        let inverted: MarkSpan = serde_json::from_value(wire).expect("inverted span deserializes");
        assert!(inverted.start > inverted.end, "fixture must be inverted");

        let out = clamp_marks_to(10, &[inverted]);
        assert!(out[0].start <= out[0].end);
        assert_eq!((out[0].start, out[0].end), (5, 5));
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
}
