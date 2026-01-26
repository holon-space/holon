use super::prelude::*;
use holon_api::render_types::RenderExpr;

pub fn render(
    content: &Box<ViewModel>,
    _compiled_sql: &Option<String>,
    _query_context_id: &Option<String>,
    _render_expr: &Option<RenderExpr>,
    _ctx: &DioxusRenderContext,
) -> Element {
    rsx! { RenderNode { node: (**content).clone() } }
}
