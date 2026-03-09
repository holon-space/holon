use super::prelude::*;
use holon_frontend::view_model::ViewKind;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Table { children } = &node.kind else {
        return rsx! {};
    };
    rsx! {
        div {
            style: "display: flex; flex-direction: column; gap: 1px;",
            for (k, child) in children.items.iter().enumerate().map(|(i, c)| (super::util::child_key(i, c), c)) {
                RenderNode { key: "{k}", node: child.clone() }
            }
        }
    }
}
