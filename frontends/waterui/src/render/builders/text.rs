use holon_api::render_eval::resolve_color_name;

use super::prelude::*;

pub fn build(ba: BA) -> AnyView {
    let content = ba
        .args
        .get_positional_string(0)
        .map(|s| s.to_string())
        .or_else(|| ba.args.get_string("content").map(|s| s.to_string()))
        .unwrap_or_else(|| {
            ba.args
                .positional
                .first()
                .map(|v| v.to_display_string())
                .unwrap_or_default()
        });

    let size = ba.args.get_f64("size").unwrap_or(14.0) as f32;
    let bold = ba.args.get_bool("bold").unwrap_or(false);
    let color = ba.args.get_string("color").map(|c| parse_color(c));

    let t = text(content).size(size);
    match (bold, color) {
        (true, Some(c)) => AnyView::new(t.bold().foreground(c)),
        (true, None) => AnyView::new(t.bold()),
        (false, Some(c)) => AnyView::new(t.foreground(c)),
        (false, None) => AnyView::new(t),
    }
}

fn parse_color(s: &str) -> Color {
    Color::srgb_hex(resolve_color_name(s))
}
