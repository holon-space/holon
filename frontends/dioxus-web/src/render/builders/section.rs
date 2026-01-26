use super::prelude::*;

pub fn render(title: &String, children: &LazyChildren, _ctx: &DioxusRenderContext) -> Element {
    let title = title.clone();
    rsx! {
        div {
            style: "display: flex; flex-direction: column; gap: 0px;",
            div {
                style: "font-weight: bold; color: #aaa; font-size: 0.85em; padding: 4px 0;",
                "{title}"
            }
            for (i, child) in children.items.iter().enumerate() {
                RenderNode { key: "{i}", node: child.clone() }
            }
        }
    }
}
