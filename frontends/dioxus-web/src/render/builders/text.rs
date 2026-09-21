use holon_frontend::view_model::ViewKind;

use super::prelude::*;

/// `size` is a resolved point value on the snapshot, so it paints here — but
/// only when it differs from the shadow builder's body default. An inline
/// style outranks every selector, so emitting it unconditionally would kill
/// the document-title rule in `index.html` and paint titles at body size.
/// The tradeoff: a `text()` that explicitly asks for 14 is indistinguishable
/// from one that asked for nothing, and defers to the stylesheet.
///
/// `text_style` is the semantic type-scale keyword (`#{style: "h1"}`),
/// carried unresolved on the snapshot and resolved here through
/// `render_eval::text_style_treatment` — the one resolver every frontend
/// calls, so size and weight cannot drift apart per platform. A title
/// therefore paints as a title wherever the `page_title` role puts it, not
/// only where `index.html`'s positional document-title rule reaches, and an
/// h1's inline `font-size` deliberately outranks that rule (same 28px scale;
/// the rule's line-height, colour and letter-spacing still apply, and it
/// keeps titling the first-child spans the role did NOT style).
///
/// The `empty` placeholder is still NOT on the snapshot — the shadow builder
/// carries it as a prop and only GPUI resolves it — so `#{empty:
/// "(untitled)"}` cannot reach this frontend.
pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Text {
        content,
        bold,
        size,
        color,
        style: text_style,
    } = &node.kind
    else {
        return rsx! {};
    };
    let treatment = text_style
        .as_deref()
        .and_then(holon_api::render_eval::text_style_treatment);
    let bold = *bold || treatment.is_some_and(|t| t.bold);
    let mut style = String::new();
    match treatment {
        Some(t) => style.push_str(&format!("font-size: {}px;", t.size)),
        None if *size != holon_frontend::view_model::default_text_size() => {
            style.push_str(&format!("font-size: {size}px;"));
        }
        None => {}
    }
    if bold {
        style.push_str("font-weight: bold;");
    }
    if let Some(c) = color {
        style.push_str(&format!("color: {c};"));
    }
    rsx! { span { style: "{style}", "{content}" } }
}
