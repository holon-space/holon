use holon_frontend::view_model::ViewKind;

use super::prelude::*;

/// App-shell columns. GPUI's `columns()` allocates per-column width from each
/// panel's `ideal_width` / `column_priority` layout hints; the dioxus snapshot
/// `ViewKind::Columns` does not carry those hints (they are dropped at the
/// projection boundary — see report), and each panel's real content arrives
/// asynchronously through its own `live_block` subscription, so at
/// root-projection time every column's content is still `Empty`/`Loading`.
///
/// The one reliable signal available here is the panel's own `block_id`, which
/// names its role in the default layout. We map that to a column role:
///
///   * a `*sidebar*`/`*left*` panel → a fixed-width navigation sidebar,
///   * a `*right*` rail → a content-sized column that collapses to nothing when
///     its query is empty (the seed's right sidebar), and
///   * everything else → a centred, width-capped reading column.
///
/// The proper fix is to plumb `ideal_width`/`column_priority` through the
/// snapshot so this doesn't depend on well-known ids; until then this keeps
/// the browser shell readable. Unknown/idless panels fall back to `main`.
pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Columns { gap, children } = &node.kind else {
        return rsx! {};
    };
    let gap = *gap;

    rsx! {
        div { class: "holon-columns", style: "gap: {gap}px;",
            for (key, child) in keyed_children(&children.items) {
                div { key: "{key}", class: "{col_class(child)}",
                    RenderNode { node: child.clone() }
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum ColRole {
    Sidebar,
    Rail,
    Main,
}

fn col_class(vm: &ViewModel) -> &'static str {
    match col_role(vm) {
        ColRole::Sidebar => "holon-col-sidebar",
        ColRole::Rail => "holon-col-rail",
        ColRole::Main => "holon-col-main",
    }
}

fn col_role(vm: &ViewModel) -> ColRole {
    // A panel with no `live_block` is a placeholder (`spacer(0)` — the seed
    // emits these for the flanks at narrow breakpoints). Collapse it instead
    // of handing it an equal flex share and squeezing the real content.
    let Some(id) = panel_block_id(vm) else {
        return ColRole::Rail;
    };
    let id = id.to_ascii_lowercase();
    if id.contains("main") {
        ColRole::Main
    } else if id.contains("right") {
        ColRole::Rail
    } else if id.contains("sidebar") || id.contains("left") || id.contains("nav") {
        ColRole::Sidebar
    } else {
        ColRole::Main
    }
}

/// Unwrap the layout wrappers a projected panel may carry (`drawer(...)`,
/// focusable, etc.) to reach the `live_block` id that names the panel.
fn panel_block_id(vm: &ViewModel) -> Option<String> {
    match &vm.kind {
        ViewKind::LiveBlock { block_id, .. } => Some(block_id.clone()),
        ViewKind::Focusable { child }
        | ViewKind::Draggable { child }
        | ViewKind::Selectable { child }
        | ViewKind::PieMenu { child, .. }
        | ViewKind::ViewModeSwitcher { child, .. }
        | ViewKind::Drawer { child, .. } => panel_block_id(child),
        ViewKind::RenderBlock { content } => panel_block_id(content),
        _ => None,
    }
}
