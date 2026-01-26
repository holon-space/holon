use super::prelude::*;

pub fn render(accent: &String, children: &LazyChildren, _ctx: &DioxusRenderContext) -> Element {
    let border = if accent.is_empty() {
        "border-left: 3px solid #444;".to_string()
    } else {
        format!("border-left: 3px solid {accent};")
    };
    rsx! {
        div {
            style: "background: #1e1e2e; padding: 8px 12px; border-radius: 4px; {border} margin: 4px 0;",
            for (i, child) in children.items.iter().enumerate() {
                RenderNode { key: "{i}", node: child.clone() }
            }
        }
    }
}
