use super::prelude::*;

pub fn render(gap: &f32, children: &LazyChildren, _ctx: &DioxusRenderContext) -> Element {
    let gap = *gap;
    rsx! {
        div {
            style: "display: flex; flex-direction: column; gap: {gap}px;",
            for (i, child) in children.items.iter().enumerate() {
                RenderNode { key: "{i}", node: child.clone() }
            }
        }
    }
}
