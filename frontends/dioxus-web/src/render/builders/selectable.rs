use holon_api::ClickModifiers;
use holon_frontend::view_model::ViewKind;

use super::prelude::*;
use crate::editor::dispatch_chain;
use crate::editor::intent_to_wire;

/// Click-action wrapper (GPUI parity: `gpui/render/builders/selectable.rs`).
/// Pre-resolves every click-bound operation on the node into a
/// modifier-set → intent map and dispatches the matching intent on click.
/// Sidebar page rows use this: `selectable(row(…), #{action:
/// navigation_focus(…)})`.
pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Selectable { child } = &node.kind else {
        return rsx! {};
    };
    let child_vm = (**child).clone();
    // Addressable by entity for the web arm's `click_entity`: rendered-text
    // carries this already, but a selectable's clickable surface is the
    // wrapper, and its child may render no text node at all (sidebar rows).
    let dom_entity_id = node.row_id().unwrap_or_default(); // ALLOW(ok): a selectable without a row_id is a non-entity surface; the empty id is deliberately unaddressable (real ids are scheme-qualified, "" cannot collide)

    let click_intents = holon_frontend::operations::click_intents(&node.operations);
    if click_intents.is_empty() {
        return rsx! { RenderNode { node: child_vm } };
    }

    rsx! {
        div {
            "data-role": "selectable",
            "data-entity-id": "{dom_entity_id}",
            style: "cursor: pointer;",
            onclick: move |evt| {
                let m = evt.modifiers();
                let modifiers = ClickModifiers {
                    shift: m.shift(),
                    alt: m.alt(),
                    cmd: m.meta(),
                    ctrl: m.ctrl(),
                };
                let Some(intent) = click_intents.get(&modifiers) else {
                    return;
                };
                tracing::info!(
                    "[selectable] click ({modifiers:?}) → {}.{}",
                    intent.entity_name,
                    intent.op_name
                );
                evt.stop_propagation();
                dispatch_chain(vec![intent_to_wire(intent)]);
            },
            RenderNode { node: child_vm.clone() }
        }
    }
}
