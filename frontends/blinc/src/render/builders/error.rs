use super::prelude::*;

pub fn build(args: &ResolvedArgs, _ctx: &RenderContext) -> Div {
    let message = args
        .get_string("message")
        .or_else(|| args.get_positional_string(0))
        .unwrap_or("Unknown error")
        .to_string();

    let theme = ThemeState::get();
    div()
        .p(8.0)
        .rounded(4.0)
        .bg(theme.color(ColorToken::ErrorBg))
        .child(
            text(message)
                .size(13.0)
                .color(theme.color(ColorToken::Error)),
        )
}
