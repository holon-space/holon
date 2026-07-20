use std::ops::Range;

use holon_api::EntityRef;
use holon_api::EntityUri;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::Value;
use holon_api::marks_from_json;
use holon_frontend::operations::OperationIntent;

use super::prelude::*;
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

    let has_links = marks.as_ref().map_or(false, |m| {
        m.iter()
            .any(|ms| matches!(ms.mark, InlineMark::Link { .. }))
    });

    let inner: AnyElement = if !has_content || !has_links {
        // Empty block or no link marks: use existing behavior.
        click_to_focus(
            &el_id,
            static_inner(&content, ctx).into_any_element(),
            block_uri,
            services,
        )
        .into_any_element()
    } else {
        link_aware_render(&content, &marks.unwrap(), &block_uri, services)
    };

    crate::geometry::tracked(
        el_id,
        inner,
        &ctx.bounds_registry,
        "rendered_text",
        Some(&row_id),
        has_content,
        Some(std::sync::Arc::from(content)),
    )
    .into_any_element()
}

/// Render content with link marks as clickable, distinct runs.
fn link_aware_render(
    content: &str,
    marks: &[MarkSpan],
    block_uri: &EntityUri,
    services: std::sync::Arc<dyn holon_frontend::reactive::BuilderServices>,
) -> AnyElement {
    let link_color = Hsla {
        h: 0.6,
        s: 0.7,
        l: 0.6,
        a: 1.0,
    };

    // Never abort on a corrupt persisted mark. A mark span outliving its
    // content ("N..M exceeds text length K") aborts EVERY paint of this block —
    // a page permanently un-openable. Disclosed degraded render: log loud with
    // the block id (fail-loud, not silent) and let `scalar_range_to_bytes`
    // clamp the offending span. The durable fix is the read-boundary
    // `canonicalize_marks_against`; this is the render-layer safety net.
    let content_chars = content.chars().count();
    for ms in marks {
        if ms.end > content_chars {
            tracing::error!(
                block_id = %block_uri.as_str(),
                mark_start = ms.start,
                mark_end = ms.end,
                content_chars,
                "rendered_text: mark span exceeds content length; rendering \
                 degraded (clamped). Corrupt persisted marks for this block."
            );
        }
    }

    let segments = build_content_segments(content, marks);

    let mut el = div()
        .flex_row()
        .w_full()
        .px(px(12.0))
        .py(px(8.0))
        .text_sm()
        .line_height(gpui::Rems(1.25));

    for seg in &segments {
        if seg.is_link {
            let target = seg
                .link_target
                .as_ref()
                .expect("link segment must have target");
            let link_text = seg.text.clone();
            let child = match target {
                EntityRef::Internal { id } => {
                    let target_id = id.to_string();
                    div()
                        .child(link_text)
                        .text_color(link_color)
                        .underline()
                        .cursor_pointer()
                        .on_mouse_down(gpui::MouseButton::Left, {
                            let s = services.clone();
                            let tid = target_id.clone();
                            move |_, _, _| {
                                s.dispatch_intent(OperationIntent::new(
                                    "navigation".into(),
                                    "focus".into(),
                                    [
                                        ("region".into(), Value::String("main".into())),
                                        ("block_id".into(), Value::String(tid.clone())),
                                    ]
                                    .into_iter()
                                    .collect(),
                                ));
                            }
                        })
                        .into_any_element()
                }
                EntityRef::External { url } => {
                    let url = url.clone();
                    div()
                        .child(link_text)
                        .text_color(link_color)
                        .underline()
                        .cursor_pointer()
                        .on_mouse_down(gpui::MouseButton::Left, {
                            let s = services.clone();
                            let u = url.clone();
                            move |_, _, _| {
                                s.dispatch_intent(OperationIntent::new(
                                    "navigation".into(),
                                    "focus".into(),
                                    [
                                        ("region".into(), Value::String("main".into())),
                                        ("block_id".into(), Value::String(u.clone())),
                                    ]
                                    .into_iter()
                                    .collect(),
                                ));
                            }
                        })
                        .into_any_element()
                }
                EntityRef::Name { name } => {
                    // Dangling link: create the page chain for this name (lazy
                    // page-create, 2026-07-10 links ruling) and navigate the main
                    // region to the new leaf, so the click feels identical to
                    // clicking a resolved link. The next render re-resolves the
                    // healed junction and this arm becomes `Internal`.
                    let target = name.clone();
                    div()
                        .child(link_text)
                        .text_color(link_color)
                        .underline()
                        .cursor_pointer()
                        .on_mouse_down(gpui::MouseButton::Left, {
                            let s = services.clone();
                            move |_, _, _| {
                                s.follow_dangling_link(target.clone(), "main".to_string());
                            }
                        })
                        .into_any_element()
                }
            };
            el = el.child(child);
        } else {
            let text = seg.text.clone();
            el = el.child(
                div()
                    .child(text)
                    .on_mouse_down(gpui::MouseButton::Left, {
                        let s = services.clone();
                        let uri = block_uri.clone();
                        move |_, _, _| {
                            s.set_focus(Some(uri.clone()));
                        }
                    })
                    .into_any_element(),
            );
        }
    }

    el.into_any_element()
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
#[allow(dead_code)]
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
}
