use super::prelude::*;
use holon_api::render_eval::resolve_color_name;

pub fn build(ba: BA<'_>) -> Element {
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

    let size = format!("{}px", ba.args.get_f64("size").unwrap_or(14.0));
    let bold = ba.args.get_bool("bold").unwrap_or(false);
    let color = ba
        .args
        .get_string("color")
        .map(|c| resolve_color_name(c).to_string());
    let weight = if bold { "bold" } else { "normal" };

    rsx! {
        span {
            font_size: "{size}",
            font_weight: "{weight}",
            color: if let Some(c) = color { c } else { "inherit".to_string() },
            {content}
        }
    }
}
