use std::sync::Arc;

use super::prelude::*;
use crate::entity_view_registry::ToggleState;
use holon_frontend::reactive_view_model::{ReactiveViewKind, ReactiveViewModel};


/// Extract a stable ID from the first child's entity data for collapse state tracking.
/// Walks into wrapper nodes (RenderBlock, LiveQuery) to find the actual entity with an "id".
fn node_id(vm: &ReactiveViewModel) -> Option<String> {
    if let Some(id) = vm.entity.get("id").and_then(|v| v.as_string()) {
        return Some(id.to_string());
    }
    match &vm.kind {
        ReactiveViewKind::RenderBlock { slot } | ReactiveViewKind::LiveQuery { slot, .. } => {
            let content = slot.content.lock_ref();
            content.entity.get("id").and_then(|v| v.as_string()).map(|s| s.to_string())
        }
        _ => None,
    }
}

fn bullet_dot(ctx: &GpuiRenderContext) -> Div {
    let s = ctx.style();
    div()
        .flex_shrink_0()
        .w(px(s.tree_chevron_size))
        .h(px(s.tree_item_min_height))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(s.tree_bullet_size))
                .h(px(s.tree_bullet_size))
                .rounded(px(s.tree_bullet_size / 2.0))
                .bg(tc(ctx, |t| t.muted_foreground)),
        )
}

fn get_or_create_toggle(ctx: &GpuiRenderContext, key: &str) -> gpui::Entity<ToggleState> {
    let any = ctx.local.get_or_create(key, || {
        ctx.with_gpui(|_window, cx| cx.new(|_cx| ToggleState { active: false }).into_any())
    });
    any.downcast().expect("cached entity type mismatch")
}

fn collapse_chevron(
    collapsed: bool,
    el_id: String,
    toggle_entity: gpui::Entity<ToggleState>,
    ctx: &GpuiRenderContext,
) -> gpui::Stateful<Div> {
    let chevron = if collapsed {
        "\u{25B6}" // ▶ right-pointing triangle
    } else {
        "\u{25BC}" // ▼ down-pointing triangle
    };
    let color = tc(ctx, |t| t.muted_foreground);

    div()
        .id(hashed_id(&format!("tree-toggle-{el_id}")))
        .cursor_pointer()
        .flex_shrink_0()
        .w(px(ctx.style().tree_chevron_size))
        .h(px(ctx.style().tree_chevron_size))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(ctx.style().tree_chevron_font_size))
        .text_color(color)
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
            toggle_entity.update(cx, |t, _cx| {
                t.active = !t.active;
            });
            window.refresh();
        })
        .child(chevron.to_string())
}

/// Check if a tree_item node is collapsed.
/// Returns `(depth, collapsed)` if the node is a TreeItem with has_children=true,
/// or `(depth, false)` for leaf tree_items. Returns None for non-tree_item nodes.
pub fn collapse_state(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Option<(usize, bool)> {
    let ReactiveViewKind::TreeItem {
        depth,
        has_children,
        children,
        ..
    } = &node.kind
    else {
        return None;
    };

    if !has_children {
        return Some((*depth, false));
    }

    let id = children.first().and_then(|c| node_id(c));
    let collapsed = id.map_or(false, |id| {
        let key = format!("tree-collapse:{id}");
        let toggle = get_or_create_toggle(ctx, &key);
        ctx.with_gpui(|_window, cx| toggle.read(cx).active)
    });
    Some((*depth, collapsed))
}

/// Flat tree item renderer.
///
/// Each tree_item carries `depth` (for indentation) and `has_children` (for chevron).
/// The single child in `children` is the content widget.
/// Collapse state is tracked per-node; the *tree collection* renderer skips
/// descendants of collapsed nodes (see `tree.rs` / `collection_view.rs`).
pub fn render(
    depth: &usize,
    has_children: &bool,
    children: &Vec<Arc<ReactiveViewModel>>,
    ctx: &GpuiRenderContext,
) -> Div {
    let items = children.clone();

    let id = items.first().and_then(|c| node_id(c));

    let collapsed = if *has_children {
        id.as_ref().map_or(false, |id| {
            let key = format!("tree-collapse:{id}");
            let toggle = get_or_create_toggle(ctx, &key);
            ctx.with_gpui(|_window, cx| toggle.read(cx).active)
        })
    } else {
        false
    };

    // Store collapse state in entity data so the tree collection can read it
    // when deciding which items to skip. We use a convention: the tree_item
    // itself is not skipped, but the tree/outline renderer checks collapse.
    let _ = collapsed; // collapse filtering happens at the collection level

    let content = items.first().map(|child| super::render(child, ctx));

    let indent = (*depth as f32) * ctx.style().tree_indent_px;

    let mut row = div()
        .w_full()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(4.0))
        .min_h(px(ctx.style().tree_item_min_height))
        .pl(px(indent));

    if *has_children {
        let el_id = id.clone().unwrap_or_else(|| "tree-toggle".to_string());
        let key = format!("tree-collapse:{el_id}");
        let toggle = get_or_create_toggle(ctx, &key);
        row = row.child(collapse_chevron(collapsed, el_id, toggle, ctx));
    } else {
        row = row.child(bullet_dot(ctx));
    }

    if let Some(node) = content {
        row = row.child(div().flex_1().child(node));
    }

    row
}
