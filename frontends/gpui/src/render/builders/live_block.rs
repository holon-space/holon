use std::sync::Arc;

use holon_api::EntityUri;

use super::prelude::*;
use crate::navigation_state::NavigationState;
use crate::views::ReactiveShell;
use holon_frontend::reactive_view_model::ReactiveViewModel;

/// Render a live_block by looking up or lazily creating a ReactiveShell entity.
///
/// Refuses to construct (or even look up) a child whose block id is already
/// on the parent's ancestor chain — A→B→A would otherwise spin up an
/// unbounded chain of new entities, since GPUI's per-entity cache is
/// parent-scoped and won't deduplicate across the cycle. The cycle check
/// fires before the cache lookup so the cycle case never enters the cache.
pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let block_id_str = node.prop_str("block_id").unwrap_or_else(|| "".to_string());
    let block_id = EntityUri::parse(&block_id_str)
        .unwrap_or_else(|_| EntityUri::block(&block_id_str));

    let bid = block_id.to_string();
    if ctx.live_block_ancestors.contains(&bid) {
        tracing::warn!(
            "[live_block] '{bid}' would create a cycle (ancestors={:?}) — rendering empty",
            ctx.live_block_ancestors.as_slice()
        );
        return div().into_any_element();
    }

    let entity = get_or_create_live_block(&block_id, ctx);

    // Note: we deliberately do NOT wrap in tracked() here. The BoundsTracker's
    // forced style (width: 100%, flex_grow: 1) is calibrated for small content
    // widgets inside column-flex lists; wrapping a whole region (row-flex child)
    // causes the wrapper to collapse to height=0 and clips all region content.
    //
    // For PANEL containers (`block:default-*` — the LeftSidebar / Main /
    // RightSidebar wrappers), we ALSO wrap in a layout-transparent tracker
    // that binds the block_id as `entity_id`. This lets PBT region queries
    // locate the panel by URI in `BoundsRegistry`. We deliberately DON'T do
    // this for non-panel live_blocks because invariants like
    // `vm-data-tracked-as-content` (sut.rs:5820) rely on `find_by_entity_id`
    // returning the CONTENT widget (rendered_text / render_entity /
    // selectable) for a block, not its live_block wrapper. The invariant
    // already excludes `block:default-*` ids from the same check (sut.rs:5791),
    // so the convention is consistent.
    if bid.starts_with("block:default-") {
        super::tag_with_entity_id(ctx, "live_block", Some(&bid), entity)
    } else {
        entity.into_any_element()
    }
}

fn get_or_create_live_block(
    block_id: &EntityUri,
    ctx: &GpuiRenderContext,
) -> gpui::Entity<ReactiveShell> {
    let key = crate::entity_view_registry::CacheKey::LiveBlock(block_id.to_string());
    let bid = block_id.to_string();
    let services: Arc<dyn holon_frontend::reactive::BuilderServices> = ctx.services.clone();
    let render_ctx = ctx.ctx.clone();
    let bounds = ctx.bounds_registry.clone();
    let uri = block_id.clone();
    // Snapshot the parent's chain so the new shell sees the right ancestor
    // set in its own renders. The render fn already refused above if the
    // child id is already on the chain.
    let ancestors = ctx.live_block_ancestors.clone();

    ctx.local.get_or_create_typed(key, || {
        ctx.with_gpui(|_window, cx| {
            let live_block = services.watch_live(&uri, services.clone());
            cx.new(|cx| {
                ReactiveShell::new_for_block(
                    bid,
                    render_ctx,
                    services,
                    live_block,
                    NavigationState::new(),
                    bounds,
                    ancestors,
                    cx,
                )
            })
        })
    })
}
