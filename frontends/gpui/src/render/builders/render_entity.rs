use gpui::{AnyView, StyleRefinement};

use super::prelude::*;
use holon_api::EntityUri;
use holon_frontend::ReactiveViewModel;

/// Render a `render_entity` node.
///
/// Cache creation lives at the row-iteration callers in `ReactiveShell`
/// (block-mode collection iterator and the `gpui::list` per-row closure)
/// because they're the only places that hold an `Arc<ReactiveViewModel>`
/// for the row — `ReactiveViewModel` is not `Clone`. This builder is the
/// dispatch fallback for `render_entity` nodes encountered elsewhere in a
/// tree: it returns a cached entity if one exists in `entity_cache`, or
/// renders the slot's content directly with the click-to-focus wrapper.
pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let slot = node.slot.as_ref().expect("render_entity requires a slot");

    if let Some(row_id) = node.row_id() {
        let cache_key = crate::entity_view_registry::CacheKey::RenderEntity(row_id);
        let entity: Option<gpui::Entity<crate::views::RenderEntityView>> = {
            let cache = ctx.local.entity_cache.read().unwrap();
            cache
                .get(&cache_key)
                .and_then(|any| any.clone().downcast::<crate::views::RenderEntityView>().ok())
        };
        if let Some(entity) = entity {
            let mut s = StyleRefinement::default();
            s.size.width = Some(gpui::relative(1.0).into());
            return AnyView::from(entity).cached(s).into_any_element();
        }
    }

    // Fallback: render directly (no entity created yet, or render_entity
    // appears outside a collection-row context).
    let content = slot.content.lock_ref();
    let child_el = super::render(&content, ctx);

    let block_id = node
        .entity()
        .get("id")
        .and_then(|v| v.as_string())
        .map(|s| EntityUri::from_raw(s));

    let Some(ref id) = block_id else {
        return child_el;
    };

    let is_focused = ctx.services().focused_block().as_ref() == Some(id);
    if is_focused {
        return child_el;
    }

    let id_for_click = id.clone();
    let el_id = format!("render-entity-{}", id);
    let services = ctx.services.clone();
    let inner = div()
        .id(hashed_id(&el_id))
        .cursor_pointer()
        .child(child_el)
        .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
            let block_id_str = id_for_click.id().to_string();
            services.set_focus(Some(id_for_click.clone()));
            // TODO: derive region from the render context (e.g. RenderContext.region)
            // instead of hard-coding "main" — needed when sidebar regions become editable.
            let mut params = std::collections::HashMap::new();
            params.insert("region".into(), holon_api::Value::String("main".into()));
            params.insert("block_id".into(), holon_api::Value::String(block_id_str));
            params.insert("cursor_offset".into(), holon_api::Value::Integer(0));
            services.dispatch_intent(holon_frontend::OperationIntent::new(
                "navigation".into(),
                "editor_focus".into(),
                params,
            ));
        })
        .into_any_element();
    crate::geometry::tracked(
        el_id,
        inner,
        &ctx.bounds_registry,
        "render_entity",
        Some(&id.to_string()),
        true,
        None,
    )
    .into_any_element()
}
