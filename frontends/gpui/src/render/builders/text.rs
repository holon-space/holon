use super::prelude::*;

pub fn render(content: &String, bold: &bool, size: &f32, color: &Option<String>, ctx: &GpuiRenderContext) -> Div {
    let mut el = div().child(content.clone()).line_height(px(22.0));
    if *bold {
        el = el.font_weight(gpui::FontWeight::SEMIBOLD);
    }
    if *size > 0.0 {
        el = el.text_size(px(*size));
    } else {
        el = el.text_size(px(15.0));
    }
    if let Some(color_name) = color {
        let c = match color_name.as_str() {
            "muted" | "secondary" => tc(ctx, |t| t.muted_foreground),
            "warning" => tc(ctx, |t| t.warning),
            "error" => tc(ctx, |t| t.danger),
            "success" => tc(ctx, |t| t.success),
            _ => tc(ctx, |t| t.foreground),
        };
        el = el.text_color(c);
    }
    el
}
