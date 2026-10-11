use holon_frontend::ReactiveViewModel;
use holon_frontend::user_driver::build_drop_intent;
use holon_frontend::user_driver::node_drag_drop_operation;

use super::prelude::*;
use crate::render::drag::DraggedBlock;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let zone = div().h(px(4.0));
    let Some(operation) = node_drag_drop_operation(node) else {
        return zone;
    };
    let operation = operation.descriptor.clone();
    let target = node
        .row_id()
        .expect("drop_zone binds its operation from a bound row, so it names one");
    let services = ctx.services.clone();
    let drop_bg = tc(ctx, |t| t.accent);

    zone.drag_over::<DraggedBlock>(move |style, _dragged, _, _| {
        style.h(px(8.0)).bg(drop_bg).rounded(px(2.0))
    })
    .on_drop(move |dragged: &DraggedBlock, _, _| {
        if let Some(intent) = build_drop_intent(&dragged.block_id, &target, &operation) {
            services.dispatch_intent(intent);
        }
    })
}
