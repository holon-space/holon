use super::prelude::*;
use holon_frontend::view_model::ViewKind;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::StateToggle { current, label, .. } = &node.kind else {
        return rsx! {};
    };
    let display = if label.is_empty() {
        current.clone()
    } else {
        label.clone()
    };
    rsx! {
        span {
            style: "cursor: pointer; font-size: 0.85em; color: #7fdf7f; padding: 1px 4px; border: 1px solid #3a3a3a; border-radius: 3px;",
            "{display}"
        }
    }
}
