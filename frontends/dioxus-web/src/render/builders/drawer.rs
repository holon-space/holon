use holon_frontend::DRAWER_TOGGLE_WIDTH;
use holon_frontend::view_model::DrawerMode;
use holon_frontend::view_model::ViewKind;

use super::prelude::*;

/// A collapsible side panel. The `columns` wrapper above allocates the width
/// from this node's `layout_hint`; everything the panel LOOKS like — surface,
/// padding, its own scroll container — belongs here, so that a column carrying
/// a spacer or an overlay drawer paints no panel chrome.
///
/// Open/closed comes off the snapshot (`ViewKind::Drawer.open`), stamped by the
/// shared shadow builder from the same view-store read GPUI performs live. A
/// closed shrink drawer collapses to the toggle strip its `layout_hint` already
/// reserves, matching GPUI's collapsed column; a closed overlay drawer paints
/// nothing, since it holds no flow space to collapse.
pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Drawer {
        child, mode, open, ..
    } = &node.kind
    else {
        return rsx! {};
    };
    let open = *open;
    let mode_str = mode.as_str();

    if !open && matches!(mode, DrawerMode::Overlay) {
        return rsx! {
            div { "data-role": "drawer", "data-drawer-mode": "{mode_str}", "data-drawer-open": "false" }
        };
    }

    // Closed: clip to the reserved toggle strip rather than dropping the child,
    // so the panel keeps its identity (and its width) in the flow.
    let style = if open {
        String::new()
    } else {
        format!("width: {DRAWER_TOGGLE_WIDTH}px; overflow: hidden;")
    };

    rsx! {
        div {
            "data-role": "drawer",
            "data-drawer-mode": "{mode_str}",
            "data-drawer-open": "{open}",
            style: "{style}",
            RenderNode { node: (**child).clone() }
        }
    }
}
