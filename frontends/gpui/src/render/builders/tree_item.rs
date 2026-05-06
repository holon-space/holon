use super::prelude::*;
use futures_signals::signal::Mutable;
use holon_frontend::reactive_view_model::ReactiveViewModel;

/// Extract a stable ID from the first child's entity data for collapse state tracking.
/// Walks into wrapper nodes (render_entity, live_query) to find the actual entity with an "id".
fn node_id(vm: &ReactiveViewModel) -> Option<String> {
    if let Some(id) = vm.entity().get("id").and_then(|v| v.as_string()) {
        return Some(id.to_string());
    }
    let name = vm.widget_name();
    match name.as_deref() {
        Some("render_entity") | Some("live_query") => {
            if let Some(ref slot) = vm.slot {
                let content = slot.content.lock_ref();
                return content
                    .entity()
                    .get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string());
            }
            None
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

fn collapse_chevron(
    collapsed: bool,
    el_id: String,
    expanded: Mutable<bool>,
    ctx: &GpuiRenderContext,
) -> gpui::Stateful<Div> {
    let chevron = if collapsed {
        "\u{25B6}" // right-pointing triangle
    } else {
        "\u{25BC}" // down-pointing triangle
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
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, _cx| {
            expanded.set(!expanded.get());
            window.refresh();
        })
        .child(chevron.to_string())
}

/// Check if a tree_item node is collapsed.
/// Returns `(depth, collapsed)` if the node is a TreeItem with has_children=true,
/// or `(depth, false)` for leaf tree_items. Returns None for non-tree_item nodes.
///
/// Reads the per-instance `expanded` Mutable on the `ReactiveViewModel` —
/// each tree_item carries its own state (set by `wrap_tree_item` in
/// `mutable_tree.rs`). Two rows wrapping the same widget id therefore
/// have independent collapse state.
pub fn collapse_state(node: &ReactiveViewModel, _ctx: &GpuiRenderContext) -> Option<(usize, bool)> {
    if node.widget_name().as_deref() != Some("tree_item") {
        return None;
    }

    let depth = node.prop_f64("depth").unwrap_or(0.0) as usize;
    let has_children = node.prop_bool("has_children").unwrap_or(false);

    if !has_children {
        return Some((depth, false));
    }

    let expanded = node.expanded.as_ref().map_or(true, |m| m.get());
    Some((depth, !expanded))
}

/// Flat tree item renderer.
///
/// Each tree_item carries `depth` (for indentation) and `has_children` (for chevron).
/// The single child in `children` is the content widget.
/// Collapse state is tracked per-node; the *tree collection* renderer skips
/// descendants of collapsed nodes (see `tree.rs` / `collection_view.rs`).
pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let depth = node.prop_f64("depth").unwrap_or(0.0) as usize;
    let has_children = node.prop_bool("has_children").unwrap_or(false);
    // Chrome props from tree builder rules: per-row override map. Defaults
    // preserve today's behaviour (bullet on leaves, chevron on parents).
    // See `tree(rules: [...])` in render_dsl + shared_tree_build in
    // render_interpreter — rule evaluation merges chrome flags into both
    // ctx.flags AND the row's tree_item props.
    let show_bullet = node.prop_bool("show_bullet").unwrap_or(true);
    let show_chevron = node.prop_bool("show_chevron").unwrap_or(has_children);
    let children = &node.children;
    let items = children.clone();

    let id = items.first().and_then(|c| node_id(c));

    // Per-instance expand/collapse state. Read the `Mutable` from the VM
    // (set by `wrap_tree_item`) so two tree_items wrapping the same id keep
    // independent state. Default to expanded if the field is absent (e.g.,
    // tree_item built outside `wrap_tree_item`).
    let expanded_handle = node.expanded.clone();
    let collapsed = if has_children && show_chevron {
        !expanded_handle.as_ref().map_or(true, |m| m.get())
    } else {
        false
    };

    let _ = collapsed; // collapse filtering happens at the collection level

    let content = items.first().map(|child| super::render(child, ctx));

    let indent = (depth as f32) * ctx.style().tree_indent_px;

    let mut row = div()
        .w_full()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(4.0))
        .min_h(px(ctx.style().tree_item_min_height))
        .pl(px(indent));

    if show_chevron && has_children {
        let el_id = id.clone().unwrap_or_else(|| "tree-toggle".to_string());
        // Fall back to a fresh standalone Mutable when the node has no
        // `expanded` field — the chevron still renders but click toggles
        // a detached cell. In practice `wrap_tree_item` always sets one.
        let mutable = expanded_handle.unwrap_or_else(|| Mutable::new(true));
        row = row.child(collapse_chevron(collapsed, el_id, mutable, ctx));
    } else if show_bullet {
        row = row.child(bullet_dot(ctx));
    }

    if let Some(node) = content {
        row = row.child(div().flex_1().child(node));
    }

    row
}
