use super::prelude::*;
use holon_frontend::view_model::ViewKind;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Section { title, children } = &node.kind else {
        return rsx! {};
    };
    let title = title.clone();
    rsx! {
        div {
            style: "display: flex; flex-direction: column; gap: 0px;",
            div {
                style: "font-weight: bold; color: #aaa; font-size: 0.85em; padding: 4px 0;",
                "{title}"
            }
            for (i, child) in children.items.iter().enumerate() {
                RenderNode { key: "{i}", node: child.clone() }
            }
        }
    }
}
