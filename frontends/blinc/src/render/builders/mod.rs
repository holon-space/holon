pub(crate) mod operation_helpers;
mod prelude;

holon_macros::builder_registry!("src/render/builders",
    skip: [prelude, operation_helpers, selectable],
    node_dispatch: Div,
    context: RenderContext
);

use blinc_app::prelude::*;
use blinc_theme::{ColorToken, ThemeState};

use super::context::RenderContext;

/// Render a ViewModel tree into a Blinc Div.
pub fn render(node: &holon_frontend::view_model::ViewModel, ctx: &RenderContext) -> Div {
    let widget = render_node(node, ctx);
    annotate(widget, node, ctx)
}

fn annotate(widget: Div, node: &holon_frontend::view_model::ViewModel, _ctx: &RenderContext) -> Div {
    if let Some(id) = node.entity_id() {
        widget.id(id)
    } else {
        widget
    }
}

fn render_unsupported(name: &str, _ctx: &RenderContext) -> Div {
    div().flex_row().gap(4.0).child(
        blinc_app::prelude::text(format!("[unsupported: {name}]"))
            .size(12.0)
            .color(ThemeState::get().color(ColorToken::TextSecondary)),
    )
}

/// Render children of a LazyChildren into Divs.
pub(crate) fn render_children(children: &holon_frontend::view_model::LazyChildren, ctx: &RenderContext) -> Vec<Div> {
    children.items.iter().map(|child| render(child, ctx)).collect()
}
