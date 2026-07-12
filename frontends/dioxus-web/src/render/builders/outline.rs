use holon_frontend::view_model::ViewKind;

use super::prelude::*;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Outline { children } = &node.kind else {
        return rsx! {};
    };
    rsx! {
        div {
            class: "holon-outline",
            style: "display: flex; flex-direction: column; gap: 2px;",
            for (key, child) in keyed_children(&children.items) {
                RenderNode { key: "{key}", node: child.clone() }
            }
        }
    }
}
