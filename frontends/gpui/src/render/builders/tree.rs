use super::prelude::*;
use holon_frontend::view_model::LazyChildren;

pub fn render(children: &LazyChildren, ctx: &GpuiRenderContext) -> Div {
    let rendered = render_children(children, ctx);
    if rendered.is_empty() {
        return div().child("[tree: empty]");
    }
    let mut container = div().flex_col().gap_0p5();
    for child in rendered {
        container = container.child(child);
    }
    container
}
