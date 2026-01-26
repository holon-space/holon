use holon_frontend::view_model::ViewKind;

use super::prelude::*;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::List { gap, children } = &node.kind else {
        return rsx! {};
    };
    let gap = *gap;
    rsx! {
        div {
            style: "display: flex; flex-direction: column; gap: {gap}px;",
            for (k, child) in children.items.iter().enumerate().map(|(i, c)| (super::util::child_key(i, c), c)) {
                RenderNode { key: "{k}", node: child.clone() }
            }
        }
    }
}
