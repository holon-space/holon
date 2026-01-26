use super::prelude::*;

pub fn render(gap: &f32, children: &LazyChildren, _ctx: &DioxusRenderContext) -> Element {
    let gap = *gap;
    rsx! {
        div {
            style: "display: flex; flex-direction: row; gap: {gap}px; align-items: flex-start; flex: 1;",
            for (i, child) in children.items.iter().enumerate() {
                div { style: "flex: 1; min-width: 0;",
                    RenderNode { key: "{i}", node: child.clone() }
                }
            }
        }
    }
}
