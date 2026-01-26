use super::prelude::*;
use holon_frontend::ViewModel;

use crate::render::tracked_div::TrackedDiv;

pub fn render(node: &ViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    use holon_frontend::view_model::NodeKind;
    let NodeKind::BlockRef {
        block_id, content, ..
    } = &node.kind
    else {
        unreachable!()
    };

    // If the EntityRegistry has a pre-created BlockRefView for this block,
    // return it directly — it subscribes to its own ViewModel stream and
    // re-renders independently of the parent tree.
    if let Some(entity) = ctx.bounds_registry.get_block_view(block_id) {
        return entity.into_any_element();
    }

    // Fallback: render inline (first frame before reconciliation, or
    // blocks not yet in the registry).
    let child_el = super::render(content, ctx);
    let tracked = TrackedDiv::new(block_id.clone(), ctx.bounds_registry.clone(), child_el);
    div().child(tracked).into_any_element()
}
