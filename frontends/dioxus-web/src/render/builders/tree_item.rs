use super::prelude::*;

pub fn render(
    depth: &usize,
    _has_children: &bool,
    children: &LazyChildren,
    _ctx: &DioxusRenderContext,
) -> Element {
    let pad = depth * 16;
    rsx! {
        div { style: "padding-left: {pad}px;",
            for (i, child) in children.items.iter().enumerate() {
                RenderNode { key: "{i}", node: child.clone() }
            }
        }
    }
}
