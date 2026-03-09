use std::sync::Arc;

use super::prelude::*;
use holon_frontend::reactive_view_model::ReactiveViewModel;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let target_id = node.prop_str("target_id").unwrap_or_else(|| "".to_string());
    let expanded = node.expanded.as_ref().expect("expand_toggle requires expanded state");
    let content_slot = node.slot.as_ref();
    let children = &node.children;
    let render_ctx = node.render_ctx.as_ref();

    // content_template is stored as a JSON-serialized RenderExpr in props
    let content_template = node.props.lock_ref().get("content_template").and_then(|v| {
        if let holon_api::Value::String(s) = v {
            serde_json::from_str::<holon_api::render_types::RenderExpr>(s).ok()
        } else {
            None
        }
    });

    let is_expanded = expanded.get();
    let chevron = if is_expanded { "\u{25BC}" } else { "\u{25B6}" };
    let color = tc(ctx, |t| t.muted_foreground);

    let expanded_handle = expanded.clone();
    let slot_handle = content_slot.map(|s| s.content.clone());
    let template = content_template;
    let captured_ctx = render_ctx.cloned();
    let services = ctx.services.clone();

    let el_id = format!("expand-toggle-{}", target_id);

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
            if new_val {
                if let (Some(ref expr), Some(ref slot), Some(ref ctx)) =
                    (&template, &slot_handle, &captured_ctx)
                {
                    let content = services.interpret(expr, ctx);
                    slot.set(Arc::new(content));
                }
            }
            window.refresh();
        })
        .child(chevron.to_string());

    let mut container = div().w_full().flex().flex_col();

    if let Some(header) = children.first() {
        let header_row = div()
            .w_full()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(4.0))
            .child(chevron_el)
            .child(div().flex_1().child(super::render(header, ctx)));
        container = container.child(header_row);
    }

    if is_expanded {
        if let Some(slot) = content_slot {
            let slot_content = slot.content.lock_ref().clone();
            container = container.child(
                div()
                    .w_full()
                    .pl(px(ctx.style().tree_indent_px))
                    .child(super::render(&slot_content, ctx)),
            );
        }
    }

    container
}
