use holon_frontend::view_model::DrawerMode;
use holon_frontend::view_model::ViewKind;

use super::prelude::*;

/// A collapsible side panel. The `columns` wrapper above allocates the width
/// from this node's `layout_hint`; everything the panel LOOKS like — surface,
/// padding, its own scroll container — belongs here, so that a column carrying
/// a spacer or an overlay drawer paints no panel chrome.
///
/// GAP vs GPUI: the drawer's open/closed state is not on the snapshot.
/// `ViewKind::Drawer` carries only `block_id`, `mode`, `width` and `child`,
/// while GPUI reads `services.drawer_open(&block_id, mode)` off the live
/// view-store. So this frontend always renders the panel open and reserves its
/// full width, where GPUI collapses a closed shrink drawer to its toggle
/// width. Closing that gap needs an `open` field on `ViewKind::Drawer` — see
/// the BugFunnel row.
pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Drawer { child, mode, .. } = &node.kind else {
        return rsx! {};
    };
    let mode = match mode {
        DrawerMode::Shrink => "shrink",
        DrawerMode::Overlay => "overlay",
    };
    rsx! {
        div { "data-role": "drawer", "data-drawer-mode": "{mode}",
            RenderNode { node: (**child).clone() }
        }
    }
}
