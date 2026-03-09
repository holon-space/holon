use gpui::{AnyView, StyleRefinement};

use super::prelude::*;
use holon_frontend::ReactiveViewModel;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let slot = node.slot.as_ref().expect("live_query requires a slot");
    let compiled_sql = node.prop_str("compiled_sql");
    let query_context_id = node.prop_str("query_context_id");
    // render_expr stored as a serialized string in props
    let render_expr_prop = node.prop_str("render_expr");

    // If we have reactive metadata AND a LiveQueryView entity in the registry, use it.
    if let (Some(sql), Some(_render_expr)) = (compiled_sql, render_expr_prop) {
        let key = super::live_query_key(&sql, query_context_id.as_deref());
        if let Some(entity) = ctx.local.live_queries.get(&key) {
            let mut s = StyleRefinement::default();
            s.flex_grow = Some(1.0);
            s.size.width = Some(gpui::relative(1.0).into());
            s.size.height = Some(gpui::relative(1.0).into());
            return AnyView::from(entity.clone())
                .cached(s)
                .into_any_element();
        }
    }

    // Fallback: render the static content snapshot.
    let content = slot.content.lock_ref();
    super::render(&content, ctx)
}
