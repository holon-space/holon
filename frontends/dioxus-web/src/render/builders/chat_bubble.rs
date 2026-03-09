use super::prelude::*;

pub fn render(
    sender: &String,
    time: &String,
    children: &LazyChildren,
    _ctx: &DioxusRenderContext,
) -> Element {
    let sender = sender.clone();
    let time = time.clone();
    rsx! {
        div { style: "margin: 4px 0; padding: 6px 10px; background: #1e1e2e; border-radius: 8px;",
            div { style: "font-size: 0.75em; color: #888; margin-bottom: 2px;",
                "{sender} · {time}"
            }
            for (i, child) in children.items.iter().enumerate() {
                RenderNode { key: "{i}", node: child.clone() }
            }
        }
    }
}
