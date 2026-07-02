use super::prelude::*;
use holon_frontend::view_model::ViewKind;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::TreeItem { depth, children, .. } = &node.kind else {
        return rsx! {};
    };
    let pad = depth * 16;
    rsx! {
        div { style: "padding-left: {pad}px;",
            for (key, child) in keyed_children(&children.items) {
                RenderNode { key: "{key}", node: child.clone() }
            }
        }
    }
}
