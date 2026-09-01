use std::collections::HashMap;
use std::sync::Arc;

use holon_api::Value;
use holon_frontend::ReactiveViewModel;
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::BuilderServices;

use super::prelude::*;
use crate::geometry::TransparentTracker;

/// What a secret preference shows instead of its value. The only place the
/// settings row is allowed to say a secret is present.
const SECRET_MASK: &str = "••••••••";

fn dispatch_set_preference(services: &Arc<dyn BuilderServices>, key: &str, value: Value) {
    services.dispatch_intent(OperationIntent {
        entity_name: "preferences".into(),
        op_name: "set".into(),
        params: HashMap::from([
            ("key".into(), Value::String(key.into())),
            ("value".into(), value),
        ]),
    });
}

/// Persist a preference and, on failure, surface a visible degraded-mode toast
/// instead of aborting. A settings write that hits a read-only config dir
/// (Android's relative `.holon` regression) must keep the app alive and tell
/// the user it did not stick — never SIGABRT the process.
fn set_preference_or_toast(
    services: &Arc<dyn BuilderServices>,
    key: &str,
    value: Value,
    cx: &mut gpui::App,
) {
    if let Err(e) = services.set_preference(key, value) {
        crate::share_ui::DegradedToastSink::push(
            crate::share_ui::DegradedToast {
                kind: crate::share_ui::DegradedKind::PreferenceSaveFailed,
                shared_tree_id: format!("preference:{key}"),
                detail: format!("Couldn't save '{key}': {e:#}"),
                condition: None,
            },
            cx,
        );
    }
}

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let key = node.prop_str("key").unwrap_or_default();
    let pref_type = node.prop_str("pref_type").unwrap_or_default();
    let value = node
        .props
        .lock_ref()
        .get("value")
        .cloned()
        .unwrap_or(Value::Null);
    let requires_restart = node.prop_bool("requires_restart").unwrap_or(false);
    let locked = node.prop_bool("locked").unwrap_or(false);
    let options: Vec<Value> = match node.props.lock_ref().get("options") {
        Some(Value::Array(arr)) => arr.clone(),
        _ => vec![],
    };
    let children = &node.children;

    let label = children
        .first()
        .and_then(|c| c.prop_str("content").map(|s| s.to_string()))
        .unwrap_or_else(|| key.clone());

    let value_str = match value {
        Value::String(ref s) => s.clone(),
        Value::Boolean(b) => if b { "on" } else { "off" }.to_string(),
        ref other => format!("{other:?}"),
    };

    let (input_el, painted) = if locked {
        build_locked_display(ctx, &pref_type, &value_str)
    } else {
        build_input(ctx, &pref_type, &value, &value_str, &key, &options)
    };

    let mut label_col = div().flex_col().flex_1().gap(px(2.0)).child(
        div()
            .text_sm()
            .font_weight(gpui::FontWeight::MEDIUM)
            .child(label),
    );

    if locked {
        label_col = label_col.child(
            div()
                .text_xs()
                .text_color(tc(ctx, |t| t.muted_foreground))
                .child("Set by CLI/environment"),
        );
    } else if requires_restart {
        label_col = label_col.child(
            div()
                .text_xs()
                .text_color(tc(ctx, |t| t.warning))
                .child("Requires restart"),
        );
    }

    div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_4()
        .py(px(6.0))
        .px(px(8.0))
        .rounded(px(6.0))
        .hover(|s| s.bg(gpui::rgba(0xffffff08)))
        .child(label_col)
        .child(tracked_value(ctx, &key, input_el, painted))
}

/// Wrap the value control so a windowed test can read the text this row
/// actually paints.
///
/// Without it the registry records no text for a preference row, and an
/// assertion that a secret never reaches the screen would pass by finding
/// nothing rather than by finding a mask.
fn tracked_value(
    ctx: &GpuiRenderContext,
    key: &str,
    input_el: Div,
    painted: Option<String>,
) -> TransparentTracker {
    let tracker = TransparentTracker::new(
        format!("pref-value-{key}"),
        "pref_field_value",
        ctx.bounds_registry.clone(),
        div().flex_shrink_0().child(input_el).into_any_element(),
    );
    match painted {
        Some(text) => tracker.with_displayed_text(text),
        None => tracker,
    }
}

/// The read-only stand-in for a field whose value comes from the CLI or the
/// environment.
///
/// A secret stays masked here as it is in the editable field: locking a field
/// says where its value comes from, never that the value may now be shown.
/// What is in force for a locked secret is the external value, not the one this
/// row carries, so the mask reads "set elsewhere" rather than claiming a value.
fn build_locked_display(
    ctx: &GpuiRenderContext,
    pref_type: &str,
    value_str: &str,
) -> (Div, Option<String>) {
    let display = match (pref_type, value_str.is_empty()) {
        ("secret", _) => SECRET_MASK.to_string(),
        (_, true) => "Not set".to_string(),
        (_, false) => value_str.to_string(),
    };

    let el = div().child(
        div()
            .text_sm()
            .px_3()
            .py_1()
            .min_w(px(160.0))
            .rounded(px(6.0))
            .bg(tc(ctx, |t| t.secondary))
            .border_1()
            .border_color(tc(ctx, |t| t.border))
            .text_color(tc(ctx, |t| t.muted_foreground))
            .opacity(0.6)
            .child(display.clone()),
    );
    (el, Some(display))
}

fn build_input(
    ctx: &GpuiRenderContext,
    pref_type: &str,
    value: &Value,
    value_str: &str,
    key: &str,
    options: &[Value],
) -> (Div, Option<String>) {
    match pref_type {
        "toggle" => (build_toggle(ctx, value, key), None),
        "choice" => (build_choice(ctx, value_str, key, options), None),
        "secret" => build_text_field(ctx, key, value_str, true),
        _ => build_text_field(ctx, key, value_str, false),
    }
}

fn extract_options(raw_options: &[Value]) -> Vec<(String, String)> {
    raw_options
        .iter()
        .filter_map(|item| {
            if let Value::Object(obj) = item {
                let v = obj.get("value").and_then(|v| v.as_string())?.to_string();
                let l = obj.get("label").and_then(|v| v.as_string())?.to_string();
                Some((v, l))
            } else {
                None
            }
        })
        .collect()
}

fn build_choice(
    ctx: &GpuiRenderContext,
    current_value: &str,
    key: &str,
    raw_options: &[Value],
) -> Div {
    use gpui_component::button::Button;
    use gpui_component::button::DropdownButton;
    use gpui_component::menu::PopupMenuItem;

    let options = extract_options(raw_options);

    let current_label = options
        .iter()
        .find(|(v, _)| v == current_value)
        .map(|(_, l)| l.as_str())
        .unwrap_or(current_value)
        .to_string();

    let el_id = format!("pref-choice-{key}");
    let options_for_menu = options.clone();
    let current_for_menu = current_value.to_string();
    let services = ctx.services.clone();
    let key_owned = key.to_string();

    div().child(
        DropdownButton::new(hashed_id(&el_id))
            .button(Button::new("pref-choice-label").label(current_label))
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (value, label) in &options_for_menu {
                    let is_current = *value == current_for_menu;
                    let services = services.clone();
                    let key = key_owned.clone();
                    let value = value.clone();
                    menu = menu.item(
                        PopupMenuItem::new(label.clone())
                            .checked(is_current)
                            .on_click(move |_, window, cx| {
                                set_preference_or_toast(
                                    &services,
                                    &key,
                                    Value::String(value.clone()),
                                    cx,
                                );
                                // Theme may have changed — re-sync
                                window.refresh();
                            }),
                    );
                }
                menu
            }),
    )
}

fn build_text_field(
    ctx: &GpuiRenderContext,
    key: &str,
    current: &str,
    is_secret: bool,
) -> (Div, Option<String>) {
    let display = if is_secret {
        if current.is_empty() {
            "Not set".to_string()
        } else {
            SECRET_MASK.to_string()
        }
    } else {
        if current.is_empty() {
            "Click to set".to_string()
        } else {
            current.to_string()
        }
    };

    let text_color = if current.is_empty() {
        tc(ctx, |t| t.muted_foreground)
    } else {
        tc(ctx, |t| t.foreground)
    };

    let services = ctx.services.clone();
    let key_owned = key.to_string();
    let current_owned = current.to_string();
    let el_id = format!("pref-text-{key}");
    let hidden = is_secret;

    let el = div().child(
        div()
            .id(hashed_id(&el_id))
            .text_sm()
            .px_3()
            .py_1()
            .min_w(px(160.0))
            .rounded(px(6.0))
            .bg(tc(ctx, |t| t.secondary))
            .border_1()
            .border_color(tc(ctx, |t| t.border))
            .text_color(text_color)
            .cursor_pointer()
            .hover(|s| s.bg(gpui::rgba(0xffffff15)))
            .child(display.clone())
            .on_mouse_down(gpui::MouseButton::Left, move |_, window, _| {
                let services = services.clone();
                let key = key_owned.clone();
                let default = current_owned.clone();
                // prompt_text_input is blocking (osascript), run on a thread
                std::thread::spawn(move || {
                    if let Some(new_val) = prompt_text_input(&key, &default, hidden) {
                        dispatch_set_preference(&services, &key, Value::String(new_val));
                    }
                });
                window.refresh();
            }),
    );
    (el, Some(display))
}

/// Show a native macOS text input dialog via osascript.
fn prompt_text_input(key: &str, default: &str, hidden: bool) -> Option<String> {
    let hidden_str = if hidden { "with hidden answer" } else { "" };
    let script = format!(
        r#"display dialog "Enter value for {key}:" default answer "{default}" {hidden_str} buttons {{"Cancel", "OK"}} default button "OK""#,
        key = key.replace('"', r#"\""#),
        default = if hidden { "" } else { default }.replace('"', r#"\""#),
        hidden_str = hidden_str,
    );
    let output = match std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            // Disclosed degradation (fail-loud rule): on iOS/Android (and
            // Linux/Windows) there is no osascript — preference text entry
            // needs a native dialog seam there. Log instead of silently
            // treating the failure as user-cancelled.
            tracing::warn!(
                "pref_field: cannot prompt for '{key}' — osascript unavailable                  \
                 on this platform ({e}); preference left unchanged"
            );
            return None;
        }
    };
    if !output.status.success() {
        return None; // user cancelled
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // osascript returns "button returned:OK, text returned:VALUE"
    stdout
        .split("text returned:")
        .nth(1)
        .map(|s| s.trim().to_string())
}

fn build_toggle(ctx: &GpuiRenderContext, value: &Value, key: &str) -> Div {
    let checked = matches!(value, Value::Boolean(true));

    let track = super::switch_track(ctx, checked);

    let services = ctx.services.clone();
    let key_owned = key.to_string();
    let new_value = !checked;
    let el_id = format!("pref-toggle-{key}");

    div().child(
        div()
            .id(hashed_id(&el_id))
            .cursor_pointer()
            .child(track)
            .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                set_preference_or_toast(&services, &key_owned, Value::Boolean(new_value), cx);
                window.refresh();
            }),
    )
}
