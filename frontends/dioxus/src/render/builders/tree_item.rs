use super::prelude::*;
use holon_frontend::view_model::ViewKind;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::TreeItem { depth, children, .. } = &node.kind else {
        return rsx! {};
    };
    let pad = depth * 16;
    rsx! {
        div { style: "padding-left: {pad}px;",
            for (i, child) in children.items.iter().enumerate() {
                RenderNode { key: "{i}", node: child.clone() }
            }
        }
    }
}
