use holon_frontend::view_model::ViewKind;

use super::prelude::*;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::TreeItem {
        depth, children, ..
    } = &node.kind
    else {
        return rsx! {};
    };
    let depth = *depth;
    let pad = depth * 20;
    // Indent guide: a faint vertical rule at each nesting level > 0.
    let guide = if depth > 0 {
        "border-left: 1px solid var(--indent-guide, rgba(255,255,255,0.08)); margin-left: 7px;"
    } else {
        ""
    };
    rsx! {
        div { style: "padding-left: {pad}px; {guide}",
            for (key, child) in keyed_children(&children.items) {
                RenderNode { key: "{key}", node: child.clone() }
            }
        }
    }
}
