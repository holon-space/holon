use super::prelude::*;

pub fn render(width: &f32, height: &f32, color: &Option<String>, _ctx: &GpuiRenderContext) -> Div {
    let mut el = div();
    if *width > 0.0 {
        el = el.w(px(*width)).flex_shrink_0();
    }
    if *height > 0.0 {
        el = el.h(px(*height)).flex_shrink_0();
    }
    if let Some(hex) = color {
        if hex.starts_with('#') && hex.len() >= 7 && hex.is_ascii() {
            let hex = hex.trim_start_matches('#');
            let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(128);
            let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(128);
            let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(128);
            let c: Hsla = gpui::rgba((r as u32) << 24 | (g as u32) << 16 | (b as u32) << 8 | 0xFF).into();
            el = el.bg(c).rounded(px(1.0));
        }
    }
    el
}
