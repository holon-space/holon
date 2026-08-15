use holon_frontend::view_model::ViewKind;

use super::prelude::*;

/// Chevron expand/collapse. GPUI parity: pure UI state (a per-node
/// `Mutable<bool>` gate, no dispatched op) — here a local Dioxus signal
/// seeded from the snapshot's `expanded`.
pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::ExpandToggle {
        target_id,
        expanded,
        content_deferred,
        children,
    } = &node.kind
    else {
        return rsx! {};
    };
    rsx! {
        ExpandToggleNode {
            key: "{target_id}",
            target_id: target_id.clone(),
            expanded_init: *expanded,
            content_deferred: *content_deferred,
            children_vm: children.items.clone(),
        }
    }
}

#[component]
fn ExpandToggleNode(
    target_id: String,
    expanded_init: bool,
    content_deferred: bool,
    children_vm: Vec<ViewModel>,
) -> Element {
    let mut open = use_signal(|| expanded_init);
    let chevron = if open() { "▾" } else { "▸" };
    rsx! {
        div {
            "data-role": "expand-toggle",
            "data-target-id": "{target_id}",
            "data-content-deferred": "{content_deferred}",
            span {
                style: "cursor: pointer; user-select: none; color: #888; display: inline-block; width: 1em;",
                onclick: move |_| {
                    let now = !open();
                    open.set(now);
                },
                "{chevron}"
            }
            if open() {
                div { style: "padding-left: 12px;",
                    for (key, child) in keyed_children(&children_vm) {
                        RenderNode { key: "{key}", node: child.clone() }
                    }
                }
            }
        }
    }
}
