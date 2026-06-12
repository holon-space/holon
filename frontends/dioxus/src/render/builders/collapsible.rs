use super::prelude::*;
use holon_frontend::view_model::ViewKind;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Collapsible {
        header,
        icon,
        children,
    } = &node.kind
    else {
        return rsx! {};
    };
    let header = header.clone();
    let icon = icon.clone();
    rsx! {
        details { style: "margin: 2px 0;",
            summary { style: "cursor: pointer; padding: 4px; user-select: none;",
                "{icon} {header}"
            }
            div { style: "padding-left: 12px;",
                for (i, child) in children.items.iter().enumerate() {
                    RenderNode { key: "{i}", node: child.clone() }
                }
            }
        }
    }
}
