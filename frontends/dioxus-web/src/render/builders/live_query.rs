use super::prelude::*;
use holon_api::render_types::RenderExpr;

pub fn render(
    content: &Box<ViewModel>,
    _: &Option<String>,
    _: &Option<String>,
    _: &Option<RenderExpr>,
    _: &DioxusRenderContext,
) -> Element {
    rsx! { RenderNode { node: (**content).clone() } }
}
