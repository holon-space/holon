use super::prelude::*;
use blinc_core::Color;

pub fn render(content: &String, bold: &bool, _ctx: &RenderContext) -> Div {
    let theme = ThemeState::get();
    let mut t = text(content.clone())
        .size(14.0)
        .color(theme.color(ColorToken::TextPrimary));
    if *bold {
        t = t.bold();
    }
    div().child(t)
}

pub fn parse_color(s: &str) -> Color {
    let hex = holon_api::render_eval::resolve_color_name(s);
    let hex = hex.trim_start_matches('#');
    let val = u32::from_str_radix(hex, 16).unwrap_or(0xFFFFFF);
    Color::from_hex(val)
}
