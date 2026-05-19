use super::prelude::*;

pub fn render(gap: &f32, children: &LazyChildren, _: &DioxusRenderContext) -> Element {
    let gap = *gap;
    rsx! {
        div {
            style: "display: flex; flex-direction: row; gap: {gap}px; align-items: flex-start; flex-wrap: wrap;",
            for (i, child) in children.items.iter().enumerate() {
                RenderNode { key: "{i}", node: child.clone() }
            }
        }
    }
}
