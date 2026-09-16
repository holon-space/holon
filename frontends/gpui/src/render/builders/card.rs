use super::prelude::*;

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

pub fn render(node: &holon_frontend::ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    // An accent is optional. With none the card takes the theme's muted
    // foreground for both its border and its tint, in place of the two
    // different hardcoded greys this used to fall through to.
    let accent_color = match node.prop_str("accent").as_deref().filter(|a| !a.is_empty()) {
        Some(name) => {
            super::theme::theme_token_color(ctx, super::theme::theme_token_from_prop(name))
        }
        None => tc(ctx, |t| t.muted_foreground),
    };
    let children = &node.children;
    let s = ctx.style();
    let border_radius = s.card_border_radius;
    let pad_x = s.card_padding_x;
    let pad_y = s.card_padding_y;
    let gap = s.card_gap;
    drop(s);

    let accent_u32 = super::theme::packed(accent_color);
    let card_bg = super::theme::packed(tc(ctx, |t| t.secondary));
    let tinted = tint_rgba(accent_u32, card_bg);

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
