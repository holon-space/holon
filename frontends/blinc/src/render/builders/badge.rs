use super::prelude::*;

pub fn render(label: &String, _ctx: &RenderContext) -> Div {
    let theme = ThemeState::get();
    div()
        .px(8.0)
        .py(2.0)
        .rounded(12.0)
        .bg(theme.color(ColorToken::SurfaceElevated))
        .child(
            text(label.clone())
                .size(11.0)
                .color(theme.color(ColorToken::TextSecondary)),
        )
}
