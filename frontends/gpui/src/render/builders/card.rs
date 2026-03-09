use std::sync::Arc;

use super::prelude::*;

fn hex_to_hsla(hex: &str) -> gpui::Hsla {
    let hex = hex.trim_start_matches('#');
    if hex.len() < 6 || !hex.is_ascii() {
        return gpui::rgba(0x888888FF).into();
    }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(128);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(128);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(128);
    gpui::rgba((r as u32) << 24 | (g as u32) << 16 | (b as u32) << 8 | 0xFF).into()
}

/// Tint an RGBA base color toward an accent at ~15% blend in linear RGB space.
fn tint_rgba(accent: u32, base: u32) -> gpui::Hsla {
    let mix = |a: u32, b: u32, shift: u32| -> u32 {
        let ca = ((a >> shift) & 0xFF) as f32;
        let cb = ((b >> shift) & 0xFF) as f32;
        (ca * 0.15 + cb * 0.85) as u32
    };
    let r = mix(accent, base, 24);
    let g = mix(accent, base, 16);
    let b = mix(accent, base, 8);
    gpui::rgba((r << 24) | (g << 16) | (b << 8) | 0xFF).into()
}

fn parse_hex_u32(hex: &str) -> u32 {
    let hex = hex.trim_start_matches('#');
    if hex.len() < 6 || !hex.is_ascii() {
        return 0x2A2A27FF;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(42) as u32;
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(42) as u32;
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(39) as u32;
    (r << 24) | (g << 16) | (b << 8) | 0xFF
}

const CARD_BG: u32 = 0x2A2A27FF;

pub fn render(
    accent: &String,
    children: &Vec<Arc<holon_frontend::reactive_view_model::ReactiveViewModel>>,
    ctx: &GpuiRenderContext,
) -> Div {
    let s = ctx.style();
    let border_radius = s.card_border_radius;
    let pad_x = s.card_padding_x;
    let pad_y = s.card_padding_y;
    let gap = s.card_gap;
    drop(s);

    let accent_u32 = parse_hex_u32(accent);
    let accent_color = hex_to_hsla(accent);
    let tinted = tint_rgba(accent_u32, CARD_BG);

    let mut container = div()
        .w_full()
        .bg(tinted)
        .rounded(px(border_radius))
        .shadow_sm()
        .border_l_4()
        .border_color(accent_color)
        .px(px(pad_x))
        .py(px(pad_y))
        .flex()
        .flex_col()
        .gap(px(gap))
        .cursor_pointer()
        .hover(|s| s.shadow_md());

    for child in render_children(children, ctx) {
        container = container.child(child);
    }

    container
}
