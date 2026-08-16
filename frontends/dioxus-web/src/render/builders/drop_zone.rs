use holon_frontend::user_driver::build_drop_intent;
use holon_frontend::view_model::ViewKind;

use super::prelude::*;
use crate::editor::dispatch_chain;
use crate::editor::intent_to_wire;

/// Drop target rendered below each block row (the block profile's
/// `default`/`editing` variants emit `column(row(…), drop_zone())`).
///
/// GPUI parity (`gpui/render/builders/drop_zone.rs`): dropping block S on
/// the zone of block T dispatches `build_drop_intent(S, T, entity, op)` —
/// params `{id: S, parent_id: T}`, op `move_block` — i.e. S becomes a child
/// of T. Same engine path as the headless `UserDriver::drop_entity`.
pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::DropZone { op_name } = &node.kind else {
        return rsx! {};
    };
    let target_id = node.row_id();
    let entity_name = node
        .entity_name()
        .unwrap_or_else(|| holon_api::EntityName::new("block"));
    rsx! {
        DropZoneNode {
            target_id,
            entity_name: entity_name.to_string(),
            op_name: op_name.clone(),
        }
    }
}

#[component]
fn DropZoneNode(target_id: Option<String>, entity_name: String, op_name: String) -> Element {
    let mut hovered = use_signal(|| false);
    let style = if hovered() {
        "height: 8px; margin: 1px 0; border-radius: 2px; background: #4a9eda;"
    } else {
        "height: 4px; margin: 1px 0; border-radius: 2px; background: transparent;"
    };
    rsx! {
        div {
            "data-role": "drop-zone",
            "data-target-id": target_id.as_deref().unwrap_or(""),
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
                let Some(target) = target_id.clone() else {
                    tracing::error!(
                        "[dnd] drop_zone without row_id — cannot dispatch {op_name} for {source_id}"
                    );
                    return;
                };
                let source = holon_api::entity_uri_from_id_str(&source_id);
                let target = holon_api::entity_uri_from_id_str(&target);
                let Some(intent) = build_drop_intent(
                    &source,
                    &target,
                    holon_api::EntityName::new(&entity_name),
                    &op_name,
                ) else {
                    return;
                };
                tracing::info!("[dnd] drop: {source_id} -> {target} via {op_name}");
                dispatch_chain(vec![intent_to_wire(&intent)]);
            },
        }
    }
}
