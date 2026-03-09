use super::prelude::*;
use holon_frontend::ViewModel;

pub fn render(node: &ViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    use holon_frontend::view_model::NodeKind;
    let NodeKind::LiveQuery {
        content,
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
        if let Some(entity) = ctx.bounds_registry.get_live_query_view(&key) {
            return entity.into_any_element();
        }
    }

    // Fallback: render the static content snapshot.
    super::render(content, ctx)
}
