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
            for (k, child) in children.items.iter().enumerate().map(|(i, c)| (super::util::child_key(i, c), c)) {
                RenderNode { key: "{k}", node: child.clone() }
            }
        }
    }
}
