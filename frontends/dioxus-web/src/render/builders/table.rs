use super::prelude::*;
use holon_frontend::view_model::ViewKind;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Table { children } = &node.kind else {
        return rsx! {};
    };
    rsx! {
        div {
            style: "display: flex; flex-direction: column; gap: 1px;",
            for (key, child) in keyed_children(&children.items) {
                RenderNode { key: "{key}", node: child.clone() }
            }
        }
    }
}
