use super::prelude::*;
use holon_frontend::view_model::ViewKind;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Badge { label, .. } = &node.kind else {
        return rsx! {};
    };
    rsx! {
        span {
            style: "display:inline-block; padding: 1px 6px; border-radius: 3px; font-size: 0.78em; background: #2a2a3a; color: #ccc; margin: 0 2px;",
            "{label}"
        }
    }
}
