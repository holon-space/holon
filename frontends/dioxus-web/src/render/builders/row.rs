use holon_frontend::view_model::ViewKind;

use super::prelude::*;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Row { gap, children } = &node.kind else {
        return rsx! {};
    };
    let gap = *gap;
    rsx! {
        div {
            class: "holon-row",
            style: "display: flex; flex-direction: row; gap: {gap}px; align-items: baseline; flex-wrap: wrap; padding: 1px 4px;",
            for (key, child) in keyed_children(&children.items) {
                RenderNode { key: "{key}", node: child.clone() }
            }
        }
    }
}
