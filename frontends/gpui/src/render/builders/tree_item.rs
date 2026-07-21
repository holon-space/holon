use std::collections::HashMap;
use std::sync::Arc;

use futures_signals::signal::Mutable;
use holon_api::Value;
use holon_frontend::OperationIntent;
use holon_frontend::expand_toggle_id_for;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::ReactiveViewModel;

use super::prelude::*;
use crate::geometry::TransparentTracker;

/// The single leading chrome element a tree row draws, if any. A row draws at
/// most ONE of these — chevron and bullet are mutually exclusive, and a row may
/// draw neither. The outline sets `show_bullet: false` on every row (the block
/// content already draws its own draggable orgmode bullet), so its leaf rows
/// resolve to [`LeadingMarker::None`] and never double up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeadingMarker {
    Chevron,
    Bullet,
    None,
}

/// Pick the row's leading marker. Kept as a pure function so the
/// "no redundant second bullet" contract is unit-testable without a window.
fn leading_marker(show_chevron: bool, has_children: bool, show_bullet: bool) -> LeadingMarker {
    if show_chevron && has_children {
        LeadingMarker::Chevron
    } else if show_bullet {
        LeadingMarker::Bullet
    } else {
        LeadingMarker::None
    }
}

/// Every tree row reserves the SAME leading-marker gutter regardless of which
/// marker it draws (chevron, bullet, or none) so that a row's content x-offset
/// is `depth * tree_indent_px + gutter + gap` — a strictly increasing function
/// of depth alone. Without a reserved gutter on marker-less rows, a parent
/// (which draws a chevron) is offset one gutter-width further right than its
/// own marker-less children, inverting the visual indent. Pure fn = window-free
/// regression guard for the indentation-inversion bug (BugFunnel 2026-07-21).
fn marker_gutter_px(style: &super::style::LayoutStyle) -> f32 {
    // Chevron and bullet both occupy a `tree_chevron_size`-wide box (see
    // `collapse_chevron` / `bullet_dot`); the empty None slot matches it.
    style.tree_chevron_size
}

/// Extract a stable ID from the first child's entity data for collapse state
/// tracking. Walks into wrapper nodes (render_entity, live_query) to find the
/// actual entity with an "id".
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

/// Resolve the row's profile and confirm it carries a `set_field` op,
/// returning the owning entity name for a typed intent (same pattern as
/// `board::resolve_row_op_entity`). Logs and returns `None` when the row has
/// no profile / op — the chevron then folds view-locally only (disclosed
/// degraded mode for static contexts like the design gallery).
fn resolve_set_field_entity(
    services: &Arc<dyn BuilderServices>,
    row_id: &str,
) -> Option<holon_api::EntityName> {
    let mut probe: HashMap<String, Value> = HashMap::new();
    probe.insert("id".into(), Value::String(row_id.to_string()));
    let Some(profile) = services.resolve_profile(&probe) else {
        tracing::warn!(
            "tree_item chevron: resolve_profile None for row_id={row_id}; collapse will not \
             persist"
        );
        return None;
    };
    let Some(op) = profile.operations.iter().find(|o| o.name == "set_field") else {
        tracing::warn!(
            "tree_item chevron: set_field op not on profile for row_id={row_id}; collapse will \
             not persist"
        );
        return None;
    };
    Some(op.entity_name.clone())
}

fn collapse_chevron(
    collapsed: bool,
    el_id: String,
    expanded: Mutable<bool>,
    persist: Option<(Arc<dyn BuilderServices>, String)>,
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
            let new_expanded = !expanded.get();
            expanded.set(new_expanded);
            // Collapse is document state: persist through the normal op path
            // (dispatcher → engine) so it is undoable, provenance-tagged
            // (set_field origin = User) and synced. The local gate flip above
            // keeps the fold instant; the CDC echo re-wraps the tree_item
            // with the same value.
            if let Some((services, row_id)) = &persist {
                if let Some(entity_name) = resolve_set_field_entity(services, row_id) {
                    let intent = OperationIntent::set_field(
                        &entity_name,
                        "set_field",
                        row_id,
                        "collapsed",
                        Value::Boolean(!new_expanded),
                    );
                    services.dispatch_intent(intent);
                }
            }
            window.refresh();
        })
        .child(chevron.to_string())
}

/// Check if a tree_item node is collapsed.
/// Returns `(depth, collapsed)` if the node is a TreeItem with
/// has_children=true, or `(depth, false)` for leaf tree_items. Returns None for
/// non-tree_item nodes.
///
/// Reads the per-instance `expanded` Mutable on the `ReactiveViewModel` —
/// each tree_item carries its own state (set by `wrap_tree_item` in
/// `mutable_tree.rs`). Two rows wrapping the same widget id therefore
/// have independent collapse state.
// ALLOW(unused_param): ctx kept in signature for future per-node theme reads
pub fn collapse_state(node: &ReactiveViewModel, _ctx: &GpuiRenderContext) -> Option<(usize, bool)> {
    if node.widget_name().as_deref() != Some("tree_item") {
        return None;
    }

    let depth = node.prop_f64("depth").unwrap_or(0.0) as usize;
    let has_children = node.prop_bool("has_children").unwrap_or(false);

    if !has_children {
        return Some((depth, false));
    }

    let expanded = node.expanded.as_ref().is_none_or(|m| m.get());
    Some((depth, !expanded))
}

/// Flat tree item renderer.
///
/// Each tree_item carries `depth` (for indentation) and `has_children` (for
/// chevron). The single child in `children` is the content widget.
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

    // Prefer an explicit `target_id` prop on the tree_item VM — generators
    // and shadow builders that need stable click-targetable chevrons stamp
    // this so the bounds-registry id is deterministic. Fall back to the
    // child's entity id (production org-tree path).
    let explicit_target = node.prop_str("target_id");
    let id = explicit_target
        .clone()
        .or_else(|| items.first().and_then(|c| node_id(c)));

    // Per-instance expand/collapse state. Read the `Mutable` from the VM
    // (set by `wrap_tree_item`) so two tree_items wrapping the same id keep
    // independent state. Default to expanded if the field is absent (e.g.,
    // tree_item built outside `wrap_tree_item`).
    let expanded_handle = node.expanded.clone();
    let collapsed = if has_children && show_chevron {
        !expanded_handle.as_ref().is_none_or(|m| m.get())
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

    match leading_marker(show_chevron, has_children, show_bullet) {
        LeadingMarker::Chevron => {
            let el_id = id.clone().unwrap_or_else(|| "tree-toggle".to_string());
            // Fall back to a fresh standalone Mutable when the node has no
            // `expanded` field — the chevron still renders but click toggles
            // a detached cell. In practice `wrap_tree_item` always sets one.
            let mutable = expanded_handle.unwrap_or_else(|| Mutable::new(true));
            // Persist through set_field(collapsed) when the row is identifiable;
            // rows without an id (synthetic gallery items) fold view-locally.
            let persist = id.clone().map(|row_id| (ctx.services.clone(), row_id));
            let chevron_el = collapse_chevron(collapsed, el_id, mutable, persist, ctx);
            if let Some(target_id) = explicit_target.as_deref() {
                // Register the chevron in the bounds registry under the
                // canonical id so layout-PBT `ToggleCollapse` transitions
                // can click it via `expand_toggle_id_for(target_id)`.
                row = row.child(TransparentTracker::new(
                    expand_toggle_id_for(target_id),
                    "expand_toggle",
                    ctx.bounds_registry.clone(),
                    chevron_el.into_any_element(),
                ));
            } else {
                row = row.child(chevron_el);
            }
        }
        LeadingMarker::Bullet => {
            row = row.child(bullet_dot(ctx));
        }
        LeadingMarker::None => {
            // Reserve the same leading-marker gutter even when this row draws
            // no chevron/bullet. Without it, content x-offset would be
            // `depth*indent + (chevron ? chevron_size+gap : 0)`: a parent (which
            // draws a chevron) is pushed one gutter-width right of its own
            // marker-less children, inverting the visual indent whenever the
            // indent step is <= the chevron gutter. An empty fixed-width slot
            // makes content-x depend ONLY on depth. See the indentation-
            // inversion BugFunnel row (2026-07-21).
            row = row.child(div().flex_shrink_0().w(px(marker_gutter_px(&ctx.style()))));
        }
    }

    if let Some(node) = content {
        row = row.child(div().flex_1().child(node));
    }

    row
}

#[cfg(test)]
mod marker_tests {
    use super::LeadingMarker;
    use super::leading_marker;
    use super::marker_gutter_px;

    #[test]
    fn markerless_row_reserves_the_full_marker_gutter() {
        // The indentation-inversion fix: a marker-less row (outline leaf,
        // show_bullet=false → LeadingMarker::None) must reserve the SAME
        // leading gutter as a chevron/bullet row. Otherwise a parent's content
        // sits one gutter-width right of its own children (inverted indent).
        // The gutter equals the chevron box width and must be non-zero.
        let style = super::super::style::LayoutStyle::default();
        assert_eq!(marker_gutter_px(&style), style.tree_chevron_size);
        assert!(marker_gutter_px(&style) > 0.0);
    }

    #[test]
    fn outline_leaf_draws_no_bullet_so_content_marker_is_not_doubled() {
        // Outline rows carry `show_bullet: false` (the block content already
        // draws its own draggable orgmode bullet). A leaf then has no chevron
        // and no tree bullet — exactly one marker total (the content's), never
        // two. This is the regression guard for the double-bullet bug.
        assert_eq!(
            leading_marker(false, false, false),
            LeadingMarker::None,
            "show_bullet=false leaf must not draw a redundant tree bullet"
        );
    }

    #[test]
    fn parent_keeps_its_disclosure_chevron() {
        // A collapsible parent still shows the chevron even with bullets
        // suppressed — the chevron is a disclosure control, not a bullet.
        assert_eq!(leading_marker(true, true, false), LeadingMarker::Chevron);
    }

    #[test]
    fn bullet_only_when_requested_and_not_a_parent() {
        // Trees that opt into bullets (default `show_bullet: true`, e.g. the
        // sidebar page tree) still get exactly one bullet on their leaves.
        assert_eq!(leading_marker(false, false, true), LeadingMarker::Bullet);
        // Chevron wins over bullet on a parent — never both.
        assert_eq!(leading_marker(true, true, true), LeadingMarker::Chevron);
    }
}
