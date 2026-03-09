use std::f32::consts::PI;

use super::prelude::*;
use crate::entity_view_registry::ToggleState;
use holon_frontend::operations::find_ops_affecting;
use holon_frontend::{OperationIntent, ReactiveViewModel};

const RADIUS: f32 = 52.0;
const ITEM_SIZE: f32 = 36.0;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    use holon_frontend::reactive_view_model::ReactiveViewKind;
    let ReactiveViewKind::PieMenu { fields, child } = &node.kind else {
        unreachable!()
    };

    let child_el = super::render(child, ctx);

    let field_list: Vec<String> = if fields == "this" || fields == "*" || fields == "this.*" {
        node.operations
            .iter()
            .flat_map(|ow| ow.descriptor.affected_fields.clone())
            .collect()
    } else {
        fields.split(',').map(|f| f.trim().to_string()).collect()
    };

    let field_refs: Vec<&str> = field_list.iter().map(|s| s.as_str()).collect();
    let ops = find_ops_affecting(&field_refs, &node.operations);
    if ops.is_empty() {
        return child_el;
    }

    let row_id = node.row_id();
    let entity_name = node.entity_name().map(str::to_string);
    let menu_id = format!("pie-menu-{}", row_id.as_deref().unwrap_or("x"));

    let key = format!("pie-open:{menu_id}");
    let any = ctx.local.get_or_create(&key, || {
        ctx.with_gpui(|_window, cx| cx.new(|_cx| ToggleState { active: false }).into_any())
    });
    let toggle: gpui::Entity<ToggleState> = any.downcast().expect("cached entity type mismatch");
    let is_open = ctx.with_gpui(|_window, cx| toggle.read(cx).active);

    let toggle_clone = toggle.clone();
    let trigger = div()
        .id(hashed_id(&menu_id))
        .child(child_el)
        .on_mouse_down(gpui::MouseButton::Right, move |_, _, cx| {
            toggle_clone.update(cx, |t, _cx| t.active = !t.active);
        });

    if !is_open {
        return div().child(trigger).into_any_element();
    }

    let n = ops.len();
    let accent = tc(ctx, |t| t.accent);
    let bg = tc(ctx, |t| t.background);
    let border = tc(ctx, |t| t.border);
    let text_color = tc(ctx, |t| t.foreground);

    let overlay_size = (RADIUS + ITEM_SIZE) * 2.0;
    let center = overlay_size / 2.0 - ITEM_SIZE / 2.0;

    let mut overlay = div()
        .absolute()
        .top(px(-(overlay_size / 2.0 - ITEM_SIZE)))
        .left(px(-(overlay_size / 2.0 - ITEM_SIZE)))
        .w(px(overlay_size))
        .h(px(overlay_size));

    for (i, op) in ops.iter().enumerate() {
        let angle = 2.0 * PI * i as f32 / n as f32 - PI / 2.0;
        let x = center + RADIUS * angle.cos();
        let y = center + RADIUS * angle.sin();

        let intent_template =
            row_id.as_ref().map(|id| OperationIntent::for_row(op, id, entity_name.as_deref()));
        let display_name = op.display_name.clone();
        let el_id = format!("pie-item-{}-{}", op.name, row_id.as_deref().unwrap_or("x"));
        let toggle_clone = toggle.clone();
        let services = ctx.services.clone();

        overlay = overlay.child(
            div()
                .id(hashed_id(&el_id))
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(ITEM_SIZE))
                .h(px(ITEM_SIZE))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(ITEM_SIZE / 2.0))
                .bg(bg)
                .border_1()
                .border_color(border)
                .shadow_sm()
                .cursor_pointer()
                .text_xs()
                .text_color(text_color)
                .hover(|s| s.bg(accent).text_color(gpui::rgb(0xffffff)))
                .child(abbreviate(&display_name))
                .on_mouse_down(gpui::MouseButton::Left, move |_, _, cx| {
                    if let Some(intent) = intent_template.clone() {
                        services.dispatch_intent(intent);
                    }
                    toggle_clone.update(cx, |t, _cx| t.active = false);
                }),
        );
    }

    let toggle_clone = toggle.clone();
    let backdrop = div()
        .id(hashed_id(&format!("{menu_id}-backdrop")))
        .absolute()
        .top(px(-1000.0))
        .left(px(-1000.0))
        .w(px(4000.0))
        .h(px(4000.0))
        .on_mouse_down(gpui::MouseButton::Left, move |_, _, cx| {
            toggle_clone.update(cx, |t, _cx| t.active = false);
        });

    div()
        .relative()
        .child(trigger)
        .child(gpui::deferred(backdrop).with_priority(9))
        .child(gpui::deferred(overlay).with_priority(10))
        .into_any_element()
}

/// Abbreviate a display name to fit in a small circle (max 3 chars).
fn abbreviate(name: &str) -> String {
    if name.len() <= 3 {
        return name.to_string();
    }
    let initials: String = name
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(3)
        .collect();
    if initials.len() >= 2 {
        initials.to_uppercase()
    } else {
        name[..3].to_string()
    }
}
