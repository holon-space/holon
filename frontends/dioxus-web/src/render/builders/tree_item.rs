use super::prelude::*;

pub fn render(
    depth: &usize,
    _: &bool,
    children: &LazyChildren,
    _: &DioxusRenderContext,
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
