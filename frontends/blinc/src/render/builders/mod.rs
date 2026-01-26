pub(crate) mod live_query;
pub(crate) mod operation_helpers;
mod prelude;

holon_macros::builder_registry!("src/render/builders",
    skip: [prelude, live_query, operation_helpers],
    dispatch: Div
);

use blinc_app::prelude::*;
use blinc_theme::{ColorToken, ThemeState};

use super::context::RenderContext;
use holon_api::render_eval::ResolvedArgs;

/// Build a Blinc Div from a render function name and resolved arguments.
pub fn build(name: &str, args: &ResolvedArgs, ctx: &RenderContext) -> Div {
    let widget = dispatch_build(name, args, ctx).unwrap_or_else(|| {
        tracing::warn!("Unknown builder: {name}");
        div().flex_row().gap(4.0).child(
            blinc_app::prelude::text(format!("[unknown: {name}]"))
                .size(12.0)
                .color(ThemeState::get().color(ColorToken::TextSecondary)),
        )
    });
    annotate(widget, name, ctx)
}

/// Tag the widget with a test ID from the row context.
///
/// Uses Blinc's `id()` method for element identification and geometry queries.
fn annotate(widget: Div, _name: &str, ctx: &RenderContext) -> Div {
    if let Some(id) = ctx.row().get("id").and_then(|v| v.as_string()) {
        widget.id(id)
    } else {
        widget
    }
}
