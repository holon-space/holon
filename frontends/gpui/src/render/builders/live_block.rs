use std::sync::Arc;

use holon_api::EntityUri;

use super::prelude::*;
use crate::navigation_state::NavigationState;
use crate::views::ReactiveShell;
use holon_frontend::reactive_view_model::ReactiveViewModel;

/// Render a live_block by looking up or lazily creating a ReactiveShell entity.
pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let block_id_str = node.prop_str("block_id").unwrap_or_else(|| "".to_string());
    let block_id = EntityUri::parse(&block_id_str)
        .unwrap_or_else(|_| EntityUri::block(&block_id_str));

    let entity = get_or_create_live_block(&block_id, ctx);

    // Note: we deliberately do NOT wrap in tracked() here. The BoundsTracker's
    // forced style (width: 100%, flex_grow: 1) is calibrated for small content
    // widgets inside column-flex lists; wrapping a whole region (row-flex child)
    // causes the wrapper to collapse to height=0 and clips all region content.
    entity.into_any_element()
}

fn get_or_create_live_block(
    block_id: &EntityUri,
    ctx: &GpuiRenderContext,
) -> gpui::Entity<ReactiveShell> {
    let key = format!("live-block-{block_id}");
    let bid = block_id.to_string();
    let services: Arc<dyn holon_frontend::reactive::BuilderServices> = ctx.services.clone();
    let render_ctx = ctx.ctx.clone();
    let focus = ctx.focus.clone();
    let bounds = ctx.bounds_registry.clone();
    let uri = block_id.clone();

    let entity = ctx.local.get_or_create(&key, || {
        ctx.with_gpui(|_window, cx| {
            let live_block = services.watch_live(&uri, services.clone());
            cx.new(|cx| {
                ReactiveShell::new_for_block(
                    bid,
                    render_ctx,
                    services,
                    live_block,
                    focus,
                    NavigationState::new(),
                    bounds,
                    cx,
                )
            })
            .into_any()
        })
    });

    entity.downcast().expect("cached entity type mismatch")
}
