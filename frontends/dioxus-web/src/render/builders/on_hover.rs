use super::prelude::*;
use holon_frontend::view_model::ViewKind;

/// Hover-reveal container. `children.items[0]` (the trigger) renders always;
/// `children.items[1..]` (the content) render only while the trigger's region
/// is hovered.
///
/// GPUI parity: hover is pure per-render-slot view state (a per-node
/// `Mutable<bool>` gate in the live tree — never a shared registry). The
/// snapshot carries no hover bit, so here each slot owns a local Dioxus
/// signal seeded to "not hovered".
pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::OnHover { children } = &node.kind else {
        return rsx! {};
    };
    let mut items = children.items.clone();
    let trigger = (!items.is_empty()).then(|| items.remove(0));
    rsx! {
        OnHoverNode { trigger, content_vm: items }
    }
}

#[component]
fn OnHoverNode(trigger: Option<ViewModel>, content_vm: Vec<ViewModel>) -> Element {
    let mut hovered = use_signal(|| false);
    rsx! {
        span {
            style: "display: inline-flex; align-items: center; gap: 8px;",
            onmouseenter: move |_| hovered.set(true),
            onmouseleave: move |_| hovered.set(false),
            if let Some(trigger) = trigger {
                RenderNode { node: trigger.clone() }
            }
            if hovered() {
                for (key, child) in keyed_children(&content_vm) {
                    RenderNode { key: "{key}", node: child.clone() }
                }
            }
        }
    }
}
