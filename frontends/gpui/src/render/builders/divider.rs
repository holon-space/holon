use holon_frontend::ReactiveViewModel;

use super::prelude::*;

pub fn render(_node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    div()
        .w_full()
        .h(px(1.0))
        .border_b_1()
        .border_color(tc(ctx, |t| t.border))
}
