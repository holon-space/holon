use holon_frontend::view_model::ViewKind;

use super::prelude::*;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Error { message, .. } = &node.kind else {
        return rsx! {};
    };
    let msg = message.clone();
    rsx! {
        div {
            style: "display: flex; align-items: flex-start; gap: 8px; \
                    background: rgba(229,102,107,0.10); \
                    border: 1px solid rgba(229,102,107,0.35); \
                    border-radius: var(--radius-sm, 4px); \
                    padding: 8px 12px; margin: 4px 0; \
                    color: var(--danger, #e5666b); font-size: 0.9em;",
            span { style: "flex-shrink: 0;", "⚠" }
            span { style: "color: var(--text-secondary, #a2a2a6);", "{msg}" }
        }
    }
}
