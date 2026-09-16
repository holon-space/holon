use holon_frontend::ReactiveViewModel;

use super::prelude::*;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let width = node.prop_f64("width").unwrap_or(0.0) as f32;
    let height = node.prop_f64("height").unwrap_or(0.0) as f32;
    let color = super::theme::optional_colour_prop(ctx, node.prop_str("color").as_deref());
    let grow = node.prop_bool("grow").unwrap_or(false);

    let mut el = div();
    if grow {
        // Elastic gap: absorbs the row's slack so everything after it sits at
        // the trailing edge. `flex_basis(0)` keeps the share independent of the
        // declared `width`, which an elastic spacer leaves at 0 anyway.
        let style = el.style();
        style.flex_grow = Some(1.0);
        style.flex_basis = Some(px(0.0).into());
        return el;
    }
    if width > 0.0 {
        el = el.w(px(width)).flex_shrink_0();
    }
    if height > 0.0 {
        el = el.h(px(height)).flex_shrink_0();
    }
    // A coloured spacer is a thin rule. It used to accept only a literal hex and
    // silently paint nothing for a token name, so `color: "muted"` drew no rule
    // at all and nothing said so.
    if let Some(c) = color {
        el = el.bg(c).rounded(px(1.0));
    }
    el
}
