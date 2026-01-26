use super::prelude::*;
use holon_frontend::view_model::ViewKind;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Error { message, .. } = &node.kind else {
        return rsx! {};
    };
    let msg = message.clone();
    rsx! { div { style: "color: #ff5252; font-size: 0.85em;", "⚠ {msg}" } }
}
