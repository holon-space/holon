use super::prelude::*;
use crate::geometry::TransparentTracker;
use holon_frontend::{expand_toggle_id_for, reactive_view_model::ReactiveViewModel};

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

    let expanded_handle = expanded.clone();
    let el_id = format!("expand-toggle-{}", target_id);

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
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, _cx| {
            let new_val = !expanded_handle.get();
            expanded_handle.set(new_val);
            // Materialisation happens lazily on the next render when
            // `materialize_if_gated()` sees the open gate. No need to
            // call `services.interpret` here.
            window.refresh();
        })
        .child(chevron.to_string());

    // Register the chevron in the bounds registry under the canonical id
    // so layout-PBT `ToggleCollapse` transitions can click it via
    // `click_at_element(expand_toggle_id_for(target_id))`.
    let tracked_chevron = TransparentTracker::new(
        expand_toggle_id_for(&target_id),
        "expand_toggle",
        ctx.bounds_registry.clone(),
        chevron_el.into_any_element(),
    );

    let mut container = div().w_full().flex().flex_col();

    if let Some(header) = children.first() {
        let header_row = div()
            .w_full()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(4.0))
            .child(tracked_chevron)
            .child(div().flex_1().child(super::render(header, ctx)));
        container = container.child(header_row);
    }

    if is_expanded {
        if let Some(content) = materialised {
            container = container.child(
                div()
                    .w_full()
                    .pl(px(ctx.style().tree_indent_px))
                    .child(super::render(&content, ctx)),
            );
        }
    }

    container
}
