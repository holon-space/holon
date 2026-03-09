use gpui::AnyView;
use gpui::StyleRefinement;
use holon_frontend::ReactiveViewModel;

use super::prelude::*;

/// Render a `render_entity` node.
///
/// Cache creation lives at the row-iteration callers in `ReactiveShell`
/// (block-mode collection iterator and the `gpui::list` per-row closure)
/// because they're the only places that hold an `Arc<ReactiveViewModel>`
/// for the row — `ReactiveViewModel` is not `Clone`. When encountered
/// elsewhere in a tree, this builder returns a cached entity if one
/// exists in `entity_cache`, or renders the slot's content directly.
/// Click-to-focus is handled by the `rendered_text` leaf inside the slot
/// (see `block_profile.yaml`).
pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let slot = node.slot.as_ref().expect("render_entity requires a slot");

    if let Some(row_id) = node.row_id() {
        let cache_key = crate::entity_view_registry::CacheKey::RenderEntity(row_id);
        let entity: Option<gpui::Entity<crate::views::RenderEntityView>> = {
            let cache = ctx.local.entity_cache.read().unwrap();
            cache.get(&cache_key).and_then(|any| {
                any.clone()
                    .downcast::<crate::views::RenderEntityView>()
                    .ok()
            }) // ALLOW(ok): downcast Err means cache entry is a different concrete type
        };
        if let Some(entity) = entity {
            let mut s = StyleRefinement::default();
            s.size.width = Some(gpui::relative(1.0).into());
            return AnyView::from(entity).cached(s).into_any_element();
        }
    }

    let content = slot.content.lock_ref();
    super::render(&content, ctx)
}
