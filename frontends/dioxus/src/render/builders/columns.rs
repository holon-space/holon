use holon_frontend::view_model::ViewKind;

use super::prelude::*;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Columns { gap, children } = &node.kind else {
        return rsx! {};
    };
    let gap = *gap;
    rsx! {
        div {
            style: "display: flex; flex-direction: row; gap: {gap}px; align-items: flex-start; flex: 1;",
            for (k, child) in children.items.iter().enumerate().map(|(i, c)| (super::util::child_key(i, c), c)) {
                div { key: "{k}", style: "flex: 1; min-width: 0;",
                    RenderNode { node: child.clone() }
                }
            }
        }
    }
}
