use gpui::{AnyView, StyleRefinement};

use super::prelude::*;
use holon_frontend::ReactiveViewModel;

/// Render a live_query node by lazily creating a `LiveQueryView` entity in
/// the parent's `entity_cache`. Falls back to rendering the static slot
/// content when the node is missing the props needed to subscribe (e.g.
/// during a transitional structural rebuild before the engine has filled
/// in `compiled_sql` / `render_expr`).
pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let slot = node.slot.as_ref().expect("live_query requires a slot");
    let compiled_sql = node.prop_str("compiled_sql");
    let query_context_id = node.prop_str("query_context_id");
    let render_expr_str = node.prop_str("render_expr");

    if let (Some(sql), Some(re_str)) = (compiled_sql, render_expr_str) {
        if let Ok(re) = serde_json::from_str::<holon_api::render_types::RenderExpr>(&re_str) {
            let key = super::live_query_key(&sql, query_context_id.as_deref());
            let cache_key = crate::entity_view_registry::CacheKey::LiveQuery(key);

            let services = ctx.services.clone();
            let nav = ctx.nav.clone();
            let bounds = ctx.bounds_registry.clone();

            let entity = ctx.local.get_or_create_typed(cache_key, || {
                let query_context = query_context_id.as_ref().map(|id| {
                    let uri = holon_api::EntityUri::from_raw(id);
                    holon_frontend::QueryContext {
                        current_block_id: Some(uri.clone()),
                        context_parent_id: Some(uri),
                        context_path_prefix: None,
                    }
                });
                let signal = services.watch_query_signal(sql, re, query_context);
                let svc = services.clone();
                let render_ctx = holon_frontend::RenderContext::default();
                ctx.with_gpui(|_window, cx| {
                    cx.new(|cx| {
                        crate::views::LiveQueryView::new(render_ctx, svc, signal, nav, bounds, cx)
                    })
                })
            });

            let mut s = StyleRefinement::default();
            s.flex_grow = Some(1.0);
            s.size.width = Some(gpui::relative(1.0).into());
            s.size.height = Some(gpui::relative(1.0).into());
            return AnyView::from(entity).cached(s).into_any_element();
        }
    }

    // Fallback: render the static content snapshot.
    let content = slot.content.lock_ref();
    super::render(&content, ctx)
}
