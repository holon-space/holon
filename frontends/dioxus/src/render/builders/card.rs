use holon_frontend::view_model::ViewKind;

use super::prelude::*;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Card { accent, children } = &node.kind else {
        return rsx! {};
    };
    let border = if accent.is_empty() {
        "border-left: 3px solid #444;".to_string()
    } else {
        format!("border-left: 3px solid {accent};")
    };
    rsx! {
        div {
            style: "background: #1e1e2e; padding: 8px 12px; border-radius: 4px; {border} margin: 4px 0;",
            for (k, child) in children.items.iter().enumerate().map(|(i, c)| (super::util::child_key(i, c), c)) {
                RenderNode { key: "{k}", node: child.clone() }
            }
        }
    }
}
