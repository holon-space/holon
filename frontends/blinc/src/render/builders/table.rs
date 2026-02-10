use super::prelude::*;

/// table() — default fallback render expression.
///
/// Renders data rows as a simple vertical list showing all columns.
pub fn build(_args: &ResolvedArgs, ctx: &RenderContext) -> Div {
    let theme = ThemeState::get();
    let mut container = div().flex_col().gap(2.0);

    if ctx.data_rows.is_empty() {
        return container.child(
            text("[empty]")
                .size(12.0)
                .color(theme.color(ColorToken::TextSecondary)),
        );
    }

    // Collect column names from first row
    let columns: Vec<String> = {
        let mut cols: Vec<String> = ctx.data_rows[0].keys().cloned().collect();
        cols.sort();
        cols
    };

    // Header
    let mut header = div().flex_row().gap(8.0);
    for col in &columns {
        header = header.child(
            div().w(120.0).child(
                text(col.clone())
                    .size(11.0)
                    .color(theme.color(ColorToken::TextSecondary)),
            ),
        );
    }
    container = container.child(header);

    // Rows
    for row in &ctx.data_rows {
        let mut row_div = div().flex_row().gap(8.0);
        for col in &columns {
            let val = row
                .get(col)
                .map(|v| v.to_display_string())
                .unwrap_or_default();
            row_div = row_div.child(
                div().w(120.0).child(
                    text(val)
                        .size(13.0)
                        .color(theme.color(ColorToken::TextPrimary)),
                ),
            );
        }
        container = container.child(row_div);
    }

    container
}
