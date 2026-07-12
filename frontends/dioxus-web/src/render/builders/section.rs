use holon_frontend::view_model::ViewKind;

use super::prelude::*;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Section { title, children } = &node.kind else {
        return rsx! {};
    };
    let title = title.clone();
    rsx! {
        div {
            style: "display: flex; flex-direction: column; gap: 0px;",
            div {
                style: "font-weight: 600; color: var(--text-muted, #6d6d72); font-size: 0.72em; letter-spacing: 0.06em; text-transform: uppercase; padding: 6px 0 2px;",
                "{title}"
            }
            for (key, child) in keyed_children(&children.items) {
                RenderNode { key: "{key}", node: child.clone() }
            }
        }
    }
}
