use std::collections::HashMap;

use super::prelude::*;
use holon_api::Value;

/// Render a tappable op affordance: icon above an accessible label.
///
/// Tap → `services.present_op(op, { id: target_id })` — present_op
/// routes id-only ops to direct dispatch and multi-param ops to the
/// popup param-collection flow.
///
/// Sighted-user fallback label sits underneath the icon because GPUI
/// has no accessibility surface yet (see V2 in the mobile-bar plan —
/// `Android TalkBack` / `iOS VoiceOver` need upstream GPUI work).
///
/// **Delete confirmation:** not yet wired. The plan allows either
/// long-press-to-confirm or tap→popup-dialog; until GPUI gives us a
/// clean long-press primitive we route `delete` the same as any other
/// id-only op. Follow-up to this PR gates on a confirmation UX decision.
pub fn render(
    node: &holon_frontend::ReactiveViewModel,
    ctx: &GpuiRenderContext,
) -> AnyElement {
    let op_name = node.prop_str("op_name").unwrap_or_else(|| "".to_string());
    let target_id = node.prop_str("target_id").unwrap_or_else(|| "".to_string());
    let display_name = node.prop_str("display_name").unwrap_or_else(|| "".to_string());
    let icon_char = op_icon_char(&op_name);
    let icon_label = if icon_char.is_empty() {
        fallback_short_label(&display_name)
    } else {
        icon_char.to_string()
    };

    let services = ctx.services.clone();
    let op_name_owned = op_name.clone();
    let target_id_owned = target_id.clone();
    let element_id = format!("op-button-{op_name}-{target_id}");

    let icon_size = ctx.style().icon_size;
    let box_padding = ctx.style().icon_box_padding;

    div()
        .id(hashed_id(&element_id))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(2.0))
        .px(px(box_padding))
        .py(px(4.0))
        .cursor_pointer()
        .child(
            div()
                .text_size(px(icon_size))
                .line_height(px(icon_size))
                .text_color(tc(ctx, |t| t.foreground))
                .child(icon_label),
        )
        .child(
            div()
                .text_size(px(10.0))
                .line_height(px(12.0))
                .text_color(tc(ctx, |t| t.muted_foreground))
                .child(display_name.clone()),
        )
        .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
            present_op_from_context(&services, &op_name_owned, &target_id_owned);
        })
        .into_any_element()
}

fn present_op_from_context(
    services: &std::sync::Arc<dyn holon_frontend::reactive::BuilderServices>,
    op_name: &str,
    target_id: &str,
) {
    let mut probe: HashMap<String, Value> = HashMap::new();
    probe.insert("id".into(), Value::String(target_id.to_string()));
    let Some(profile) = services.resolve_profile(&probe) else {
        tracing::warn!(
            "op_button tap: resolve_profile returned None for target_id={target_id}"
        );
        return;
    };
    let Some(op) = profile.operations.into_iter().find(|o| o.name == op_name) else {
        tracing::warn!(
            "op_button tap: op '{op_name}' not found on profile for target_id={target_id}"
        );
        return;
    };
    let mut ctx_params: HashMap<String, Value> = HashMap::new();
    ctx_params.insert("id".into(), Value::String(target_id.to_string()));
    services.present_op(op, ctx_params);
}

/// Hardcoded op-name → single-char icon map. Unknowns return empty and
/// the caller falls back to the first two letters of `display_name`.
fn op_icon_char(op_name: &str) -> &'static str {
    match op_name {
        "cycle_task_state" => "\u{27F3}", // ⟳
        "delete" => "\u{1F5D1}",           // 🗑
        "create" => "+",
        "update" | "set_field" => "\u{270E}", // ✎
        "embed_entity" | "embed" => "\u{29C9}", // ⧉
        "indent" => "\u{21E5}",                // ⇥
        "outdent" => "\u{21E4}",               // ⇤
        "move_up" => "\u{2191}",               // ↑
        "move_down" => "\u{2193}",             // ↓
        _ => "",
    }
}

fn fallback_short_label(display_name: &str) -> String {
    display_name
        .chars()
        .filter(|c| c.is_alphanumeric())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}
