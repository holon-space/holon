use holon_frontend::user_driver::drag_drop_operation;
use holon_frontend::view_model::ViewKind;

use super::prelude::*;

/// Drag source for a block row. GPUI parity (`gpui/render/builders/
/// draggable.rs`): the wrapped child (the bullet icon in the default block
/// template) becomes the drag handle for the whole block. The dragged block
/// id is parked in `crate::dnd`; `drop_zone` builds the intent. A draggable
/// that binds no operation renders its child undraggable.
pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Draggable { child, .. } = &node.kind else {
        return rsx! {};
    };
    let child_vm = (**child).clone();

    if drag_drop_operation(&node.operations).is_none() {
        return rsx! { RenderNode { node: child_vm } };
    }
    let block_id = node
        .row_id()
        .expect("draggable binds its operation from a bound row, so it names one");

    // The typed id travels to `dnd`; the DOM attribute is text, so it is
    // spelled out here and nowhere upstream.
    let dom_block_id = block_id.to_string();

    rsx! {
        div {
            "data-role": "draggable",
            "data-block-id": "{dom_block_id}",
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
