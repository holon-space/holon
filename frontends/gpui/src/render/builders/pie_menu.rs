use std::collections::HashMap;
use std::f32::consts::PI;

use super::prelude::*;
use holon_api::Value;
use holon_frontend::ViewModel;

use super::operation_helpers::{entity_name_from_node, find_ops_affecting, row_id_from_node};
use holon_frontend::operations::dispatch_operation;

const RADIUS: f32 = 52.0;
const ITEM_SIZE: f32 = 36.0;

pub fn render(node: &ViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    use holon_frontend::view_model::NodeKind;
    let NodeKind::PieMenu { fields, child } = &node.kind else {
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

    let row_id = row_id_from_node(node);
    let entity_name = entity_name_from_node(node);
    let menu_id = format!("pie-menu-{}", row_id.as_deref().unwrap_or("x"));
    let is_open = ctx.bounds_registry.is_pie_menu_open(&menu_id);

    // Right-click on child toggles the pie menu
    let bounds_reg = ctx.bounds_registry.clone();
    let toggle_id = menu_id.clone();
    let trigger = div()
        .id(ElementId::Name(menu_id.clone().into()))
        .child(child_el)
        .on_mouse_down(gpui::MouseButton::Right, move |_, _, _| {
            bounds_reg.toggle_pie_menu(toggle_id.clone());
        });

    if !is_open {
        return div().child(trigger).into_any_element();
    }

    // Build the circular overlay
    let n = ops.len();
    let accent = tc(ctx, |t| t.accent);
    let bg = tc(ctx, |t| t.background);
    let border = tc(ctx, |t| t.border);
    let text_color = tc(ctx, |t| t.foreground);

    // Center offset: items are positioned relative to the center of the child,
    // shifted by half the overlay area. We use a fixed-size overlay centered on
    // the trigger.
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

        let session = ctx.session().clone();
        let handle = ctx.runtime_handle().clone();
        let row_id_clone = row_id.clone();
        let ent_name = entity_name
            .clone()
            .unwrap_or_else(|| op.entity_name.to_string());
        let op_name = op.name.clone();
        let display_name = op.display_name.clone();
        let el_id = format!("pie-item-{}-{}", op_name, row_id.as_deref().unwrap_or("x"));
        let bounds_reg = ctx.bounds_registry.clone();
        let close_id = menu_id.clone();

        overlay = overlay.child(
            div()
                .id(ElementId::Name(el_id.into()))
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
                .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
                    let Some(ref id) = row_id_clone else { return };
                    let mut params = HashMap::new();
                    params.insert("id".to_string(), Value::String(id.clone()));
                    dispatch_operation(
                        &handle,
                        &session,
                        ent_name.clone(),
                        op_name.clone(),
                        params,
                    );
                    bounds_reg.close_pie_menu(&close_id);
                }),
        );
    }

    // Backdrop: clicking outside closes the menu
    let bounds_reg = ctx.bounds_registry.clone();
    let close_id = menu_id.clone();
    let backdrop = div()
        .id(ElementId::Name(format!("{menu_id}-backdrop").into()))
        .absolute()
        .top(px(-1000.0))
        .left(px(-1000.0))
        .w(px(4000.0))
        .h(px(4000.0))
        .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
            bounds_reg.close_pie_menu(&close_id);
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
    // Use first letter of each word, up to 3
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
