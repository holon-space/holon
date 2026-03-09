use super::prelude::*;

pub fn render(content: &String, bold: &bool, size: &f32, color: &Option<String>, ctx: &GpuiRenderContext) -> Div {
    let mut el = div().child(content.clone()).line_height(px(26.0));
    if *bold {
        el = el.font_weight(gpui::FontWeight::SEMIBOLD);
    }
    if *size > 0.0 {
        el = el.text_size(px(*size));
    } else {
        el = el.text_size(px(15.0));
    }
    if let Some(color_name) = color {
        let c = if color_name.starts_with('#') {
            // Hex color support for accent colors
            let hex = color_name.trim_start_matches('#');
            if hex.len() >= 6 && hex.is_ascii() {
                let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(128);
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(128);
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(128);
                gpui::rgba((r as u32) << 24 | (g as u32) << 16 | (b as u32) << 8 | 0xFF).into()
            } else {
                tc(ctx, |t| t.foreground)
            }
        } else {
            match color_name.as_str() {
                "muted" | "secondary" => tc(ctx, |t| t.muted_foreground),
                "warning" => tc(ctx, |t| t.warning),
                "error" => tc(ctx, |t| t.danger),
                "success" => tc(ctx, |t| t.success),
                _ => tc(ctx, |t| t.foreground),
            }
        };
        el = el.text_color(c);
    }
    el
}
