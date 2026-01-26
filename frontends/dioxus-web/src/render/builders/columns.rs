use super::prelude::*;
use holon_frontend::view_model::ViewKind;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Columns { gap, children } = &node.kind else {
        return rsx! {};
    };
    let gap = *gap;
    rsx! {
        div {
            style: "display: flex; flex-direction: row; gap: {gap}px; align-items: flex-start; flex: 1;",
            for (key, child) in keyed_children(&children.items) {
                div { key: "{key}", style: "flex: 1; min-width: 0;",
                    RenderNode { node: child.clone() }
                }
            }
        }
    }
}
