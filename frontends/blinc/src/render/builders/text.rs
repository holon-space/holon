use super::prelude::*;
use blinc_core::Color;

pub fn build(args: &ResolvedArgs, _ctx: &RenderContext) -> Div {
    let content = args
        .get_positional_string(0)
        .map(|s| s.to_string())
        .or_else(|| args.get_string("content").map(|s| s.to_string()))
        .unwrap_or_else(|| {
            args.positional
                .first()
                .map(|v| v.to_display_string())
                .unwrap_or_default()
        });

    let size = args.get_f64("size").unwrap_or(14.0) as f32;
    let bold = args.get_bool("bold").unwrap_or(false);
    let color = if let Some(c) = args.get_string("color") {
        parse_color(c)
    } else {
        ThemeState::get().color(ColorToken::TextPrimary)
    };

    let mut t = text(content).size(size).color(color);
    if bold {
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
