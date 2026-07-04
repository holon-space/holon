use holon_frontend::view_model::ViewKind;

use super::prelude::*;

/// Drag source for a block row. GPUI parity (`gpui/render/builders/
/// draggable.rs`): the wrapped child (the bullet icon in the default block
/// template) becomes the drag handle for the whole block. The dragged block
/// id is parked in `crate::dnd`; `drop_zone` builds the intent.
pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Draggable { child, .. } = &node.kind else {
        return rsx! {};
    };
    let child_vm = (**child).clone();

    let Some(block_id) = node.row_id() else {
        // No row id — nothing to move. Render the child undraggable rather
        // than a drag that could never dispatch.
        tracing::warn!("[dnd] draggable without row_id — rendering child only");
        return rsx! { RenderNode { node: child_vm } };
    };

    rsx! {
        div {
            "data-role": "draggable",
            "data-block-id": "{block_id}",
            draggable: true,
            style: "cursor: grab; display: inline-block;",
            ondragstart: move |_| {
                crate::dnd::start_drag(block_id.clone());
            },
            ondragend: move |_| {
                crate::dnd::clear_drag();
            },
            RenderNode { node: child_vm.clone() }
        }
    }
}
