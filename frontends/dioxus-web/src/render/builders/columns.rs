use holon_frontend::LayoutHint;
use holon_frontend::view_model::ViewKind;

use super::prelude::*;

/// App-shell columns. Width allocation comes from each child's `layout_hint`,
/// the same snapshot field GPUI's `columns()` and the TUI already allocate
/// from: the shared `drawer` builder declares `Fixed`, `spacer` declares its
/// own width, and everything else keeps the `Flex { weight: 1.0 }` default.
/// `LayoutHint` maps onto CSS flex directly — `Fixed { px }` is `flex: 0 0
/// Npx` and `Flex { weight }` is `flex: weight 1 0`.
///
/// The wrapper is purely structural: it carries the flex shorthand and nothing
/// else. A `Fixed { px: 0 }` child — a `spacer(0)` flank, or an overlay drawer
/// whose contract is to claim no flow space — must therefore measure 0px, so
/// the wrapper can never own padding, background or a border under
/// `box-sizing: border-box`. Panel chrome belongs to the panel: the `drawer`
/// builder paints it.
pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Columns { gap, children } = &node.kind else {
        return rsx! {};
    };
    let gap = *gap;

    rsx! {
        div { class: "holon-columns", style: "gap: {gap}px;",
            for (key, child) in keyed_children(&children.items) {
                div {
                    key: "{key}",
                    "data-role": "column",
                    "data-layout": "{col_layout(child.layout_hint)}",
                    style: "{col_flex(child.layout_hint)}",
                    RenderNode { node: child.clone() }
                }
            }
        }
    }
}

/// Styling hook for the two allocation modes. Deliberately not a role name —
/// `Fixed` means "this column was given an exact width", not "this column is a
/// sidebar"; a spacer and an overlay drawer are `Fixed` too.
fn col_layout(hint: LayoutHint) -> &'static str {
    match hint {
        LayoutHint::Fixed { .. } => "fixed",
        LayoutHint::Flex { .. } => "flex",
    }
}

fn col_flex(hint: LayoutHint) -> String {
    match hint {
        LayoutHint::Fixed { px } => format!("flex: 0 0 {px}px;"),
        LayoutHint::Flex { weight } => format!("flex: {weight} 1 0;"),
    }
}
