use super::prelude::*;
use holon_frontend::view_model::ViewKind;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::ChatBubble {
        sender,
        time,
        children,
    } = &node.kind
    else {
        return rsx! {};
    };
    let sender = sender.clone();
    let time = time.clone();
    rsx! {
        div { style: "margin: 4px 0; padding: 6px 10px; background: #1e1e2e; border-radius: 8px;",
            div { style: "font-size: 0.75em; color: #888; margin-bottom: 2px;",
                "{sender} · {time}"
            }
            for (k, child) in children.items.iter().enumerate().map(|(i, c)| (super::util::child_key(i, c), c)) {
                RenderNode { key: "{k}", node: child.clone() }
            }
        }
    }
}
