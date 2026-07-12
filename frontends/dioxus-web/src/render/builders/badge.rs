use holon_frontend::view_model::ViewKind;

use super::prelude::*;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Badge { label, .. } = &node.kind else {
        return rsx! {};
    };
    rsx! {
        span {
            style: "display:inline-block; padding: 1px 7px; border-radius: 10px; font-size: 0.75em; font-weight: 500; background: var(--surface-elevated, #252527); color: var(--text-secondary, #a2a2a6); border: 1px solid var(--border, rgba(255,255,255,0.09)); margin: 0 2px;",
            "{label}"
        }
    }
}
