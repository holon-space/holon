use std::sync::Arc;

use super::prelude::*;
use holon_frontend::vms_button_id_for;
use holon_frontend::ReactiveViewModel;
use holon_frontend::{collection_variant_of, extract_item_template, variants_match};

use crate::geometry::TransparentTracker;

struct ModeDesc {
    name: String,
    icon: String,
}

fn parse_modes(json: &str) -> Vec<ModeDesc> {
    let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(json) else {
        return vec![];
    };
    arr.into_iter()
        .filter_map(|v| {
            let name = v.get("name")?.as_str()?.to_string();
            let icon = v.get("icon")?.as_str()?.to_string();
            Some(ModeDesc { name, icon })
        })
        .collect()
}

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let entity_uri_str = node.prop_str("entity_uri").unwrap_or_else(|| "unknown".to_string());
    let entity_uri = holon_api::EntityUri::from_raw(&entity_uri_str);
    let modes = node.prop_str("modes").unwrap_or_else(|| "[]".to_string());
    let slot = node.slot.as_ref().expect("view_mode_switcher requires a slot");

    // active_mode and mode_templates are stored in props
    let active_mode_prop = node.prop_str("active_mode").unwrap_or_else(|| "".to_string());
    let render_ctx = node.render_ctx.as_ref();

    // Shadow builder stores mode templates as individual `tmpl_mode_*` props,
    // each a JSON-serialized RenderExpr. Reconstruct the mode → expr map.
    let mode_templates: std::collections::HashMap<String, holon_api::render_types::RenderExpr> = {
        let props = node.props.lock_ref();
        props
            .iter()
            .filter_map(|(k, v)| {
                let mode_key = k.strip_prefix("tmpl_")?;
                if let holon_api::Value::String(s) = v {
                    serde_json::from_str::<holon_api::render_types::RenderExpr>(s)
                        .ok()
                        .map(|expr| (mode_key.to_string(), expr))
                } else {
                    None
                }
            })
            .collect()
    };

    let slot_content = slot.content.lock_ref().clone();
    let child_el = super::render(&slot_content, ctx);

    let mode_list = parse_modes(&modes);
    if mode_list.is_empty() {
        return child_el;
    }

    // Use a Mutable for active_mode tracking if we have one stored,
    // otherwise create one from the prop value
    let active_mode = futures_signals::signal::Mutable::new(active_mode_prop);
    let active = active_mode.get_cloned();

    let icon_size = 14.0;
    let mut icons_row = div().flex().items_center().gap(px(2.0));

    for mode in &mode_list {
        let is_active = mode.name == active;
        let tracked_id = vms_button_id_for(&entity_uri.to_string(), &mode.name);
        let gpui_el_id = format!("vms-{}-{}", entity_uri.id(), mode.name);

        let active_mode_handle = active_mode.clone();
        let slot_handle = slot.content.clone();
        let mode_templates_clone = mode_templates.clone();
        let captured_ctx = render_ctx.cloned();
        let services = ctx.services.clone();
        let mode_for_click = mode.name.clone();
        let icon_el = super::icon::render_icon(&mode.icon, icon_size, ctx);

        let button = div()
            .id(hashed_id(&gpui_el_id))
            .cursor_pointer()
            .p(px(2.0))
            .rounded(px(3.0))
            .when(is_active, |s| {
                s.bg(tc(ctx, |t| t.accent).opacity(0.15))
            })
            .when(!is_active, |s| {
                s.opacity(0.0).hover(|h| h.opacity(1.0))
            })
            .child(icon_el)
            .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                cx.stop_propagation();
                tracing::info!(
                    "[VMS_CLICK] mode={mode_for_click:?} available={:?}",
                    mode_templates_clone.keys().collect::<Vec<_>>(),
                );
                active_mode_handle.set(mode_for_click.clone());
                let template_key = format!("mode_{}", mode_for_click);

                if let Some(new_expr) = mode_templates_clone.get(&template_key) {
                    // Fast path: intra-variant switch via set_template.
                    // Extract ReactiveView + check variant in a scoped block,
                    // then DROP the ReadGuard before any write path.
                    let fast_path = {
                        let slot_content = slot_handle.lock_ref();
                        slot_content.collection.as_ref().and_then(|rv| {
                            let current_layout = rv.layout();
                            let target_layout = collection_variant_of(new_expr);
                            if variants_match(current_layout, target_layout) {
                                Some(rv.clone())
                            } else {
                                None
                            }
                        })
                    };

                    if let Some(rv) = fast_path {
                        if let Some(item_template) = extract_item_template(new_expr) {
                            rv.set_template(item_template);
                            window.refresh();
                            return;
                        }
                    }

                    // Fallback: full rebuild (cross-variant or no collection)
                    if let Some(ref ctx) = captured_ctx {
                        let content = services.interpret(new_expr, ctx);
                        let rt = services.runtime_handle();
                        let svc_arc: Arc<dyn holon_frontend::reactive::BuilderServices> =
                            services.clone();
                        holon_frontend::reactive_view::start_reactive_views(
                            &content, &svc_arc, &rt,
                        );
                        slot_handle.set(Arc::new(content));
                    }
                }
                window.refresh();
            });

        let tracked_button = TransparentTracker::new(
            tracked_id,
            "vms_button",
            ctx.bounds_registry.clone(),
            button.into_any_element(),
        );

        icons_row = icons_row.child(tracked_button);
    }

    let switcher_bar = div()
        .absolute()
        .top_0()
        .right_0()
        .pr(px(4.0))
        .pt(px(2.0))
        .child(icons_row);

    let slot_wrapper = div().flex_1().relative().child(
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(child_el),
    );

    div()
        .size_full()
        .flex()
        .flex_col()
        .relative()
        .child(slot_wrapper)
        .child(switcher_bar)
        .into_any_element()
}
