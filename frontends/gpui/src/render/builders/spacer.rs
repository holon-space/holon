use super::prelude::*;

pub fn render(width: &f32, height: &f32, _ctx: &GpuiRenderContext) -> Div {
    let mut el = div();
    if *width > 0.0 {
        el = el.w(px(*width)).flex_shrink_0();
    }
    if *height > 0.0 {
        el = el.h(px(*height)).flex_shrink_0();
    }
    el
}
