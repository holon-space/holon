use holon_frontend::view_model::ViewKind;

use super::prelude::*;

/// `size` is a resolved point value on the snapshot, so it paints here — but
/// only when it differs from the shadow builder's body default. An inline
/// style outranks every selector, so emitting it unconditionally would kill
/// the document-title rule in `index.html` and paint titles at body size.
/// The tradeoff: a `text()` that explicitly asks for 14 is indistinguishable
/// from one that asked for nothing, and defers to the stylesheet.
///
/// The `style` keyword and the `empty` placeholder are NOT on the snapshot:
/// the shadow builder carries them as props and GPUI resolves them at paint
/// time, but `ViewKind::Text` has no field for either, so `#{style: "h1"}`
/// and `#{empty: "(untitled)"}` cannot reach this frontend at all. Giving
/// them a home needs a snapshot-shape change, which is an open decision.
pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Text {
        content,
        bold,
        size,
        color,
    } = &node.kind
    else {
        return rsx! {};
    };
    let mut style = String::new();
    if *size != holon_frontend::view_model::default_text_size() {
        style.push_str(&format!("font-size: {size}px;"));
    }
    if *bold {
        style.push_str("font-weight: bold;");
    }
    if let Some(c) = color {
        style.push_str(&format!("color: {c};"));
    }
    rsx! { span { style: "{style}", "{content}" } }
}
