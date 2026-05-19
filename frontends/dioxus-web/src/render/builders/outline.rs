use super::prelude::*;

pub fn render(children: &LazyChildren, _: &DioxusRenderContext) -> Element {
    rsx! {
        div {
            style: "display: flex; flex-direction: column; gap: 0px;",
            for (i, child) in children.items.iter().enumerate() {
                RenderNode { key: "{i}", node: child.clone() }
            }
        }
    }
}
