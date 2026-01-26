use super::prelude::*;

/// source_editor(language:"holon_prql", content:"...") -- bare source code editor.
pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> Div {
    let theme = ThemeState::get();

    let language = args.get_string("language").unwrap_or("text").to_string();
    let content = args.get_string("content").unwrap_or("").to_string();

    let mut container = div().flex_col().gap(4.0);

    // Language badge
    container = container.child(
        text(language)
            .size(11.0)
            .color(theme.color(ColorToken::TextSecondary)),
    );

    if let Some(widget) = super::operation_helpers::editable_source_widget("source", &content, ctx)
    {
        container.child(widget)
    } else {
        container.child(
            text(content)
                .size(13.0)
                .color(theme.color(ColorToken::TextPrimary)),
        )
    }
}
