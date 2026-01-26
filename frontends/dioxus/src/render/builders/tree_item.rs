use holon_frontend::view_model::ViewKind;

use super::prelude::*;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::TreeItem {
        depth, children, ..
    } = &node.kind
    else {
        return rsx! {};
    };
    let pad = depth * 16;
    rsx! {
        div { style: "padding-left: {pad}px;",
            for (k, child) in children.items.iter().enumerate().map(|(i, c)| (super::util::child_key(i, c), c)) {
                RenderNode { key: "{k}", node: child.clone() }
            }
        }
    }
}
