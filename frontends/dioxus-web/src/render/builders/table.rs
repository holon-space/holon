use super::prelude::*;

pub fn render(children: &LazyChildren, _ctx: &DioxusRenderContext) -> Element {
    rsx! {
        div {
            style: "display: flex; flex-direction: column; gap: 1px;",
            for (i, child) in children.items.iter().enumerate() {
                RenderNode { key: "{i}", node: child.clone() }
            }
        }
    }
}
