use std::collections::HashMap;
use std::sync::Arc;

use holon_api::ClickModifiers;
use holon_frontend::FrontendSession;
use holon_frontend::operations::OperationIntent;
use holon_frontend::view_model::ViewKind;

use super::dispatch::click_modifiers;
use super::dispatch::dispatch_intent;
use super::prelude::*;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Selectable { .. } = &node.kind else {
        return rsx! {};
    };
    rsx! { SelectableNode { node: node.clone() } }
}

/// Click-to-operation wrapper. Mirrors
/// `gpui/src/render/builders/selectable.rs`: pre-resolves every modifier-bound
/// intent on the node into a `HashMap<ClickModifiers, OperationIntent>` and
/// dispatches the match on mouse-down. A modifier click `stop_propagation`s so
/// an outer row click doesn't also fire (LogSeq-style "pin to sidebar without
/// focusing").
#[component]
fn SelectableNode(node: ViewModel) -> Element {
    // Hooks first (unconditional) so the early returns below can't reorder them.
    let session: Arc<FrontendSession> = use_context();
    let rt: tokio::runtime::Handle = use_context();

    let ViewKind::Selectable { child } = &node.kind else {
        return rsx! {};
    };
    let child_node = (**child).clone();

    let click_intents: HashMap<ClickModifiers, OperationIntent> = node
        .operations
        .iter()
        .filter_map(|ow| {
            ow.descriptor.click_modifiers().map(|m| {
                (
                    m,
                    OperationIntent::new(
                        ow.descriptor.entity_name.clone(),
                        ow.descriptor.name.clone(),
                        ow.descriptor.bound_params.clone(),
                    ),
                )
            })
        })
        .collect();

    // No bound operations → pure passthrough (no wrapper, no cursor).
    if click_intents.is_empty() {
        return rsx! { RenderNode { node: child_node } };
    }

    rsx! {
        div {
            style: "cursor: pointer;",
            onmousedown: move |evt| {
                let mods = click_modifiers(evt.modifiers());
                if let Some(intent) = click_intents.get(&mods) {
                    if !mods.is_none() {
                        evt.stop_propagation();
                    }
                    dispatch_intent(&rt, &session, intent.clone());
                }
            },
            RenderNode { node: child_node }
        }
    }
}
