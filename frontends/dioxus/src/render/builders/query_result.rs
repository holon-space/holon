use super::prelude::*;
use holon_frontend::view_model::ViewKind;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::QueryResult { children } = &node.kind else {
        return rsx! {};
    };
    rsx! {
        div {
            style: "display: flex; flex-direction: column; gap: 0px;",
            for (i, child) in children.items.iter().enumerate() {
                RenderNode { key: "{i}", node: child.clone() }
            }
        }
    }
}
