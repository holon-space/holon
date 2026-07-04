use holon_api::Value;
use holon_frontend::OperationIntent;
use holon_frontend::expand_toggle_id_for;
use holon_frontend::operations::find_set_field_op;
use holon_frontend::reactive_view_model::ReactiveViewModel;

use super::prelude::*;
use crate::geometry::TransparentTracker;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let target_id = node.prop_str("target_id").unwrap_or_default();
    let expanded = node
        .expanded
        .as_ref()
        .expect("expand_toggle requires expanded state");
    let children = &node.children;

    let is_expanded = expanded.get();
    let chevron = if is_expanded { "\u{25BC}" } else { "\u{25B6}" };
    let color = tc(ctx, |t| t.muted_foreground);

    // Trailing hover-reveal mode: chevron sits to the RIGHT of the header and
    // is visible only while the header row is hovered. Kept in layout (opacity
    // gated, not conditionally rendered) so its bounds stay registered — PBT
    // `ToggleCollapse` and hit-testing still locate it. Ruling 2026-07-21.
    let hover_reveal = node.prop_bool("hover_reveal_toggle").unwrap_or(false);
    let is_hovered = node.hovered.as_ref().map(|h| h.get()).unwrap_or(false);
    let chevron_opacity = if hover_reveal && !is_hovered {
        0.0
    } else {
        1.0
    };

    let expanded_handle = expanded.clone();
    let seed_target = target_id.clone();
    let el_id = format!("expand-toggle-{}", target_id);

    // Dispatch the toggle as a real `set_field(collapsed)` op through the
    // normal op path (dispatcher -> engine), so collapse is undoable,
    // provenance-tagged (origin=User) and syncs as document state — not a
    // view-local poke. `None` when no `set_field` op is wired for this row
    // (e.g. static design-gallery contexts) — the chevron still folds
    // locally, it just won't persist.
    let set_field_op = find_set_field_op("collapsed", &node.operations)
        .map(|op| (node.entity_name(), op.name.clone()));
    let row_id = node.row_id();
    let services = ctx.services.clone();

    // Lazy content: `materialize_if_gated` reads the gate (= `expanded`) and
    // either returns the cached materialised VM, fires the thunk on first
    // expand and caches, or returns None while collapsed-and-empty.
    // Subsequent toggles short-circuit on the cache.
    let materialised = node
        .lazy_slot
        .as_ref()
        .and_then(|s| s.materialize_if_gated());

    let chevron_el = div()
        .id(hashed_id(&el_id))
        .cursor_pointer()
        .flex_shrink_0()
        .w(px(ctx.style().tree_chevron_size))
        .h(px(ctx.style().tree_item_min_height))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(ctx.style().tree_chevron_font_size))
        .text_color(color)
        .opacity(chevron_opacity)
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, _cx| {
            let new_val = !expanded_handle.get();
            expanded_handle.set(new_val);
            // The `Mutable` above lives only as long as THIS view model, and a
            // profile-driven row is re-synthesized on every resolve. The gate's
            // durable authority is the view-local expansion store the builder
            // seeds from, so the click has to land there too or the next
            // structural rebuild discards it.
            services.set_block_expanded_view(&seed_target, new_val);
            // Materialisation happens lazily on the next render when
            // `materialize_if_gated()` sees the open gate. No need to
            // call `services.interpret` here.
            if let (Some((Some(entity_name), op_name)), Some(id)) =
                (set_field_op.clone(), row_id.clone())
            {
                let intent = OperationIntent::set_field(
                    &entity_name,
                    &op_name,
                    &id,
                    "collapsed",
                    Value::Boolean(!new_val),
                );
                services.dispatch_intent(intent);
            }
            window.refresh();
        })
        .child(chevron.to_string());

    // Register the chevron under the canonical id so layout-PBT
    // `ToggleCollapse` and the drivers can click it via
    // `click_at_element(expand_toggle_id_for(target_id))`. The glyph travels
    // with it because bounds alone cannot tell open from closed.
    let tracked_chevron = TransparentTracker::new(
        expand_toggle_id_for(&target_id),
        "expand_toggle",
        ctx.bounds_registry.clone(),
        chevron_el.into_any_element(),
    )
    .with_vm_node(Some(target_id.as_str()))
    .with_displayed_text(chevron.to_string());

    let mut container = div().w_full().flex().flex_col();

    if let Some(header) = children.first() {
        let header_el = div().flex_1().child(super::render(header, ctx));
        let header_row = if hover_reveal {
            // Header first, chevron trailing; the whole row is the hover zone
            // that reveals the (already laid-out) chevron. `on_hover` flips the
            // node's shared `hovered` Mutable and refreshes — the same idiom
            // the `on_hover` builder uses (GPUI has no descendant-hover style).
            let hovered = node
                .hovered
                .as_ref()
                .expect("hover_reveal_toggle requires hovered state");
            let hovered_handle = hovered.clone();
            let hover_id = format!("expand-toggle-hover-{}", target_id);
            div()
                .id(hashed_id(&hover_id))
                .w_full()
                .flex()
                .flex_row()
                .items_start()
                .gap(px(4.0))
                .on_hover(move |is_over, window, _cx| {
                    if hovered_handle.get() != *is_over {
                        hovered_handle.set(*is_over);
                        window.refresh();
                    }
                })
                .child(header_el)
                .child(tracked_chevron)
                .into_any_element()
        } else {
            div()
                .w_full()
                .flex()
                .flex_row()
                .items_start()
                .gap(px(4.0))
                .child(tracked_chevron)
                .child(header_el)
                .into_any_element()
        };
        container = container.child(header_row);
    }

    if is_expanded {
        if let Some(content) = materialised {
            // This container is content-sized (`flex_col`, no height), so a
            // `live_query`'s `Panel` shape — shell `size_full` plus
            // `height: relative(1.0)` — has no definite height to resolve
            // against and collapses to 0 px, taking every row with it: the
            // nested page opens onto nothing. Render it content-height, the
            // same routing `column::render` uses for the same reason.
            let content_el = if content.widget_name().as_deref() == Some("live_query") {
                super::live_query::render_content_height(&content, ctx)
            } else {
                super::render(&content, ctx)
            };
            container = container.child(
                div()
                    .w_full()
                    .pl(px(ctx.style().tree_indent_px))
                    .child(content_el),
            );
        }
    }

    container
}
