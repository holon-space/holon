use gpui::{AnyView, StyleRefinement};

use super::prelude::*;
use holon_frontend::ReactiveViewModel;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    use holon_frontend::reactive_view_model::ReactiveViewKind;
    let ReactiveViewKind::LiveQuery {
        slot,
        compiled_sql,
        query_context_id,
        render_expr,
        ..
    } = &node.kind
    else {
        unreachable!()
    };

    // If we have reactive metadata AND a LiveQueryView entity in the registry, use it.
    if let (Some(sql), Some(_render_expr)) = (compiled_sql, render_expr) {
        let key = super::live_query_key(sql, query_context_id.as_deref());
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
