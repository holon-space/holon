use holon_api::EntityUri;
use holon_api::render_types::OperationDescriptor;
use holon_frontend::user_driver::build_drop_intent;
use holon_frontend::user_driver::drag_drop_operation;
use holon_frontend::view_model::ViewKind;

use super::prelude::*;
use crate::editor::dispatch_chain;
use crate::editor::intent_to_wire;

/// Drop target rendered below each block row (the block profile's
/// `default`/`editing` variants emit `column(row(…), drop_zone())`).
///
/// GPUI parity (`gpui/render/builders/drop_zone.rs`): dropping block S on
/// the zone of block T dispatches the zone's bound operation through
/// `build_drop_intent(S, T, op)` — params `{id: S, parent_id: T}` — i.e. S
/// becomes a child of T. A zone that binds no operation offers no drop.
pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::DropZone { op } = &node.kind else {
        return rsx! {};
    };
    let Some(operation) = drag_drop_operation(&node.operations, op) else {
        return rsx! { div { style: "height: 4px; margin: 1px 0;" } };
    };
    let target_id = node
        .row_id()
        .expect("drop_zone binds its operation from a bound row, so it names one");
    rsx! {
        DropZoneNode {
            target_id,
            operation: operation.descriptor.clone(),
        }
    }
}

#[component]
fn DropZoneNode(target_id: EntityUri, operation: OperationDescriptor) -> Element {
    let mut hovered = use_signal(|| false);
    // DOM attributes are text; the typed id is what the drop dispatches on.
    let dom_target_id = target_id.to_string();
    let style = if hovered() {
        "height: 8px; margin: 1px 0; border-radius: 2px; background: #4a9eda;"
    } else {
        "height: 4px; margin: 1px 0; border-radius: 2px; background: transparent;"
    };
    rsx! {
        div {
            "data-role": "drop-zone",
            "data-target-id": dom_target_id,
            style: "{style}",
            // dragover must be cancelled to mark the element as a valid
            // drop target (HTML5 DnD contract); only light up for drags
            // that carry one of our blocks.
            ondragover: move |evt| {
                if crate::dnd::current_drag().is_some() {
                    evt.prevent_default();
                    hovered.set(true);
                }
            },
            ondragleave: move |_| hovered.set(false),
            ondrop: move |evt| {
                evt.prevent_default();
                hovered.set(false);
                let Some(source_id) = crate::dnd::current_drag() else {
                    tracing::warn!("[dnd] drop without an active drag — ignoring");
                    return;
                };
                crate::dnd::clear_drag();
                let Some(intent) = build_drop_intent(&source_id, &target_id, &operation) else {
                    return;
                };
                tracing::info!("[dnd] drop: {source_id} -> {target_id} via {}", operation.name);
                dispatch_chain(vec![intent_to_wire(&intent)]);
            },
        }
    }
}
