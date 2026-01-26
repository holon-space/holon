use gpui::{AnyView, StyleRefinement};

use super::prelude::*;
use holon_api::EntityUri;
use holon_frontend::ReactiveViewModel;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let slot = node.slot.as_ref().expect("render_entity requires a slot");

    // Check for a pre-created RenderEntityView entity (from CollectionView).
    if let Some(row_id) = node.row_id() {
        if let Some(entity) = ctx.local.render_entitys.get(&row_id) {
            let mut s = StyleRefinement::default();
            s.size.width = Some(gpui::relative(1.0).into());
            return AnyView::from(entity.clone())
                .cached(s)
                .into_any_element();
        }
    }

    // Fallback: render directly (not inside a CollectionView, or first frame).
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
            // Update DB cursor so cursor signal doesn't override with stale position.
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
    crate::geometry::tracked(el_id, inner, &ctx.bounds_registry, "render_entity", Some(&id.to_string()), true).into_any_element()
}
