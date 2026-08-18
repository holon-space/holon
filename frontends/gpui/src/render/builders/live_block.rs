use std::sync::Arc;

use holon_api::EntityUri;
use holon_frontend::reactive_view_model::ReactiveViewModel;

use super::prelude::*;
use crate::views::ReactiveShell;

/// Render a live_block by looking up or lazily creating a ReactiveShell entity.
///
/// Refuses to construct (or even look up) a child whose block id is already
/// on the parent's ancestor chain — A→B→A would otherwise spin up an
/// unbounded chain of new entities, since GPUI's per-entity cache is
/// parent-scoped and won't deduplicate across the cycle. The cycle check
/// fires before the cache lookup so the cycle case never enters the cache.
pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let block_id_str = node.prop_str("block_id").unwrap_or_default();
    let block_id =
        EntityUri::parse(&block_id_str).unwrap_or_else(|_| EntityUri::block(&block_id_str));

    let bid = block_id.to_string();
    if ctx.live_block_ancestors.contains(&bid) {
        tracing::warn!(
            "[live_block] '{bid}' would create a cycle (ancestors={:?}) — rendering empty",
            ctx.live_block_ancestors.as_slice()
        );
        return div().into_any_element();
    }

    let entity = get_or_create_live_block(&block_id, ctx);

    // For PANEL containers (`block:default-*` — the LeftSidebar / Main /
    // RightSidebar wrappers), we wrap in a layout-transparent tracker
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
    // The slot this live_block is being placed into decides the shell's shape.
    // A panel wrapper inherits `Panel` from the window root; a `live_block()`
    // inside an outline row inherits `Nested` from the row's context.
    let placement = ctx.placement;
    // The AMBIENT router, not a fresh one: `InputRouter` is where the window
    // installed the root tree and the live-block resolver, and every input a
    // block's editor bubbles (`Navigate` for the arrows, `KeyChord` for
    // Tab/Shift+Tab/Alt+Up/Alt+Down) is answered from it. A per-shell router
    // has no root, so it answers `None` to everything.
    let nav = ctx.nav.clone();

    ctx.local.get_or_create_typed(key, || {
        ctx.with_gpui(|_window, cx| {
            let live_block = services.watch_live(&uri, services.clone());
            cx.new(|cx| {
                ReactiveShell::new_for_block(
                    bid, render_ctx, services, live_block, nav, bounds, ancestors, placement, cx,
                )
            })
        })
    })
}
