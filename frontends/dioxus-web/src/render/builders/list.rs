use super::prelude::*;
use holon_frontend::view_model::ViewKind;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::List { gap, children } = &node.kind else {
        return rsx! {};
    };
    let gap = *gap;
    rsx! {
        div {
            style: "display: flex; flex-direction: column; gap: {gap}px;",
            for (key, child) in keyed_children(&children.items) {
                RenderNode { key: "{key}", node: child.clone() }
            }
        }
    }
}
