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
            for (i, child) in children.items.iter().enumerate() {
                RenderNode { key: "{i}", node: child.clone() }
            }
        }
    }
}
