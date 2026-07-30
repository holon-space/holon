use std::collections::HashMap;
use std::sync::Arc;

use futures_signals::signal::Mutable;
use holon_api::Value;
use holon_frontend::OperationIntent;
use holon_frontend::disclosure_halo_id_for;
use holon_frontend::expand_toggle_id_for;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::ReactiveViewModel;

use super::prelude::*;
use crate::geometry::TransparentTracker;

/// A tree row is top-aligned and grows downward as its content wraps, so its
/// leading chrome must center on the FIRST LINE BOX, never on the row. Both
/// markers therefore occupy a slot exactly one line tall and center their glyph
/// inside it: the glyph lands at `row.y + text_line_height / 2` whether the row
/// is one line or five, with no hand-tuned offset to drift out of calibration.
fn first_line_slot(ctx: &GpuiRenderContext) -> Div {
    div()
        .flex_shrink_0()
        .h(px(ctx.style().text_line_height))
        .flex()
        .items_center()
        .justify_center()
}

/// A parent's disclosure affordance paints at full strength — it is the thing
/// the eye scans the sidebar for.
pub const DISCLOSURE_WEIGHT: f32 = 1.0;

/// A leaf's tree bullet is decoration, not state, so it paints BELOW disclosure
/// weight. At equal weight the (solid) bullet out-inks the (outline) chevron
/// and the hierarchy reads inverted — leaves louder than their parents (dogfood
/// F1, 2026-07-30).
pub const LEAF_BULLET_WEIGHT: f32 = 0.55;

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
    let (gutter, size) = (s.tree_chevron_size, s.tree_bullet_size);
    drop(s);
    first_line_slot(ctx)
        .opacity(LEAF_BULLET_WEIGHT)
        .w(px(gutter))
        .child(
            div()
                .w(px(size))
                .h(px(size))
                .rounded(px(size / 2.0))
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

/// The disclosure glyph a parent row draws: right-pointing while its subtree is
/// hidden, down-pointing while it is shown. Pure so the direction contract is
/// unit-testable without a window, and so the render tree can be tagged with
/// exactly the glyph it painted.
const CHEVRON_COLLAPSED: &str = "\u{25B6}";
const CHEVRON_EXPANDED: &str = "\u{25BC}";

fn chevron_glyph(collapsed: bool) -> &'static str {
    if collapsed {
        CHEVRON_COLLAPSED
    } else {
        CHEVRON_EXPANDED
    }
}

/// The glyph plus, when the row is collapsed, the halo behind it: a muted
/// filled circle that makes "this row is hiding something" scannable down the
/// whole sidebar without reading each triangle. Fills the chevron box exactly,
/// so it is background paint and never moves the row.
///
/// `halo_id` registers the halo in the bounds registry ONLY while it is
/// painted, making its presence the observable an affordance invariant reads.
fn chevron_face(
    collapsed: bool,
    halo_id: Option<String>,
    ctx: &GpuiRenderContext,
) -> gpui::AnyElement {
    // A square box of its own (not `size_full`): the enclosing slot is one text
    // line tall, and the collapsed halo below must round to a CIRCLE, not an
    // oval stretched to the line height.
    let face = div()
        .w(px(ctx.style().tree_chevron_size))
        .h(px(ctx.style().tree_chevron_size))
        .flex()
        .items_center()
        .justify_center()
        .child(chevron_glyph(collapsed).to_string());

    if !collapsed {
        return face.into_any_element();
    }

    // Filled chip with the glyph knocked out. `muted_foreground` IS the theme's
    // `text_secondary` (see `apply_holon_theme`), which
    // `theme::collapsed_halo_fill` names as the halo token and the contrast
    // test holds to a 3:1 floor against the sidebar surface in every theme.
    let face = face
        .rounded_full()
        .bg(tc(ctx, |t| t.muted_foreground))
        .text_color(tc(ctx, |t| t.background));
    match halo_id {
        Some(id) => TransparentTracker::new(
            id,
            "disclosure_halo",
            ctx.bounds_registry.clone(),
            face.into_any_element(),
        )
        .into_any_element(),
        None => face.into_any_element(),
    }
}

fn collapse_chevron(
    collapsed: bool,
    el_id: String,
    expanded: Mutable<bool>,
    persist: Option<(Arc<dyn BuilderServices>, String)>,
    halo_id: Option<String>,
    ctx: &GpuiRenderContext,
) -> gpui::Stateful<Div> {
    let color = tc(ctx, |t| t.muted_foreground);

    first_line_slot(ctx)
        .id(hashed_id(&format!("tree-toggle-{el_id}")))
        .cursor_pointer()
        .w(px(ctx.style().tree_chevron_size))
        .text_size(px(ctx.style().tree_chevron_font_size))
        .text_color(color)
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, _cx| {
            let t_toggle = std::time::Instant::now();
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
            // Feeds the p95 interaction→projection-visible SLO: without a stage
            // here the sidebar fold is invisible to `measure_latency`.
            tracing::info!(
                target: "holon_latency",
                stage = "sidebar_toggle",
                block = persist.as_ref().map(|(_, id)| id.as_str()).unwrap_or("<view-local>"),
                expanded = new_expanded,
                ms = t_toggle.elapsed().as_millis() as u64,
                "holon_latency",
            );
        })
        .child(chevron_face(collapsed, halo_id, ctx))
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
pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
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

    let marker = leading_marker(show_chevron, has_children, show_bullet);

    let mut row = div()
        .w_full()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(4.0))
        .min_h(px(ctx.style().tree_item_min_height))
        .pl(px(indent));

    match marker {
        LeadingMarker::Chevron => {
            let el_id = id.clone().unwrap_or_else(|| "tree-toggle".to_string());
            // Fall back to a fresh standalone Mutable when the node has no
            // `expanded` field — the chevron still renders but click toggles
            // a detached cell. In practice `wrap_tree_item` always sets one.
            let mutable = expanded_handle.unwrap_or_else(|| Mutable::new(true));
            // Persist through set_field(collapsed) when the row is identifiable;
            // rows without an id (synthetic gallery items) fold view-locally.
            let persist = id.clone().map(|row_id| (ctx.services.clone(), row_id));
            // Register under the row's OWN identity — the explicit `target_id`
            // when a blueprint stamped one, otherwise the content child's entity
            // id, which is all a production row carries. Keying this off
            // `target_id` alone left every real sidebar row untracked: the
            // observables existed only for synthetic fixtures, so a PBT could not
            // see the affordance disappear from the live app (dogfood F2).
            let halo_id = id.as_deref().map(disclosure_halo_id_for);
            let chevron_el = collapse_chevron(collapsed, el_id, mutable, persist, halo_id, ctx);
            if let Some(target_id) = id.as_deref() {
                // Canonical id so layout-PBT `ToggleCollapse` and
                // `UserDriver::set_block_expanded` can click it via
                // `expand_toggle_id_for(target_id)`.
                row = row.child(
                    TransparentTracker::new(
                        expand_toggle_id_for(target_id),
                        "expand_toggle",
                        ctx.bounds_registry.clone(),
                        chevron_el.into_any_element(),
                    )
                    .with_displayed_text(chevron_glyph(collapsed))
                    // The disclosure affordance is PERSISTENT: a parent row is
                    // identifiable without hovering it (and touch has no hover
                    // at all). Recorded so affordance invariants can tell a
                    // painted chevron from a merely laid-out one.
                    .with_opacity(DISCLOSURE_WEIGHT),
                );
            } else {
                row = row.child(chevron_el);
            }
        }
        LeadingMarker::Bullet => {
            let bullet = bullet_dot(ctx);
            match id.as_deref() {
                Some(target_id) => {
                    row = row.child(TransparentTracker::new(
                        holon_frontend::tree_bullet_id_for(target_id),
                        "tree_bullet",
                        ctx.bounds_registry.clone(),
                        bullet.into_any_element(),
                    ));
                }
                None => row = row.child(bullet),
            }
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
        // `min_w(0)` is load-bearing: a flex item defaults to `min-width: auto`
        // (its content's intrinsic min size), so without this the content
        // wrapper refuses to shrink below the natural width of a long line and
        // the `w_full` text inside never gets a bounded width to wrap against —
        // long block content ran off the right edge instead of reflowing
        // (BugFunnel 2026-07-22, dogfood). With `min_w(0)` the wrapper can
        // shrink to the row's available width and the text wraps.
        row = row.child(div().flex_1().min_w(px(0.0)).child(node));
    }

    row.into_any_element()
}

#[cfg(test)]
mod marker_tests {
    use super::CHEVRON_COLLAPSED;
    use super::CHEVRON_EXPANDED;
    use super::LeadingMarker;
    use super::chevron_glyph;
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

    #[test]
    fn chevron_direction_follows_collapse_state() {
        // The disclosure glyph IS the collapse state: right while the subtree
        // is hidden, down while it is shown. Persistent — no hover input can
        // reach this function, because the affordance no longer has one.
        assert_eq!(chevron_glyph(true), CHEVRON_COLLAPSED);
        assert_eq!(chevron_glyph(false), CHEVRON_EXPANDED);
    }
}
