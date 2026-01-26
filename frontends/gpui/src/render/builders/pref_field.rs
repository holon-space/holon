use super::prelude::*;
use holon_api::Value;
use holon_frontend::ViewModel;
use holon_frontend::preferences::PrefKey;

pub fn render(node: &ViewModel, ctx: &GpuiRenderContext) -> Div {
    use holon_frontend::view_model::NodeKind;
    let NodeKind::PrefField {
        key,
        pref_type,
        value,
        requires_restart,
        options,
        children,
    } = &node.kind
    else {
        unreachable!()
    };

    let label = children
        .items
        .first()
        .and_then(|c| {
            if let holon_frontend::view_model::NodeKind::Text { content, .. } = &c.kind {
                Some(content.clone())
            } else {
                None
            }
        })
        .unwrap_or_else(|| key.clone());

    let value_str = match value {
        Value::String(s) => s.clone(),
        Value::Boolean(b) => if *b { "on" } else { "off" }.to_string(),
        other => format!("{other:?}"),
    };

    let input_el = build_input(ctx, pref_type, value, &value_str, key, options);

    let mut label_col = div()
        .flex_col()
        .flex_1()
        .gap(px(2.0))
        .child(
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(label),
        );

    if *requires_restart {
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
        .child(div().flex_shrink_0().child(input_el))
}

fn build_input(
    ctx: &GpuiRenderContext,
    pref_type: &str,
    value: &Value,
    value_str: &str,
    key: &str,
    options: &[Value],
) -> Div {
    match pref_type {
        "toggle" => build_toggle(ctx, value, key),
        "choice" => build_choice(ctx, value_str, key, options),
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
    use gpui_component::button::{Button, DropdownButton};
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
    let session = ctx.session().clone();
    let key_owned = key.to_string();

    div().child(
        DropdownButton::new(ElementId::Name(el_id.into()))
            .button(Button::new("pref-choice-label").label(current_label))
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (value, label) in &options_for_menu {
                    let is_current = *value == current_for_menu;
                    let session = session.clone();
                    let key = key_owned.clone();
                    let value = value.clone();
                    menu = menu.item(
                        PopupMenuItem::new(label.clone())
                            .checked(is_current)
                            .on_click(move |_, window, cx| {
                                let pref_key = PrefKey::new(&key);
                                session.set_preference(
                                    &pref_key,
                                    toml::Value::String(value.clone()),
                                );
                                let is_dark = crate::is_theme_dark(&session);
                                let mode = if is_dark {
                                    gpui_component::theme::ThemeMode::Dark
                                } else {
                                    gpui_component::theme::ThemeMode::Light
                                };
                                gpui_component::theme::Theme::change(mode, Some(window), cx);
                                window.refresh();
                            }),
                    );
                }
                menu
            }),
    )
}

fn build_text_field(ctx: &GpuiRenderContext, key: &str, current: &str, is_secret: bool) -> Div {
    let display = if is_secret {
        if current.is_empty() { "Not set".to_string() } else { "••••••••".to_string() }
    } else {
        if current.is_empty() { "Click to set".to_string() } else { current.to_string() }
    };

    let text_color = if current.is_empty() {
        tc(ctx, |t| t.muted_foreground)
    } else {
        tc(ctx, |t| t.foreground)
    };

    let session = ctx.session().clone();
    let key_owned = key.to_string();
    let current_owned = current.to_string();
    let handle = ctx.runtime_handle().clone();
    let el_id = format!("pref-text-{key}");
    let hidden = is_secret;

    div().child(div()
        .id(ElementId::Name(el_id.into()))
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
        .child(display)
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, _| {
            let session = session.clone();
            let key = key_owned.clone();
            let default = current_owned.clone();
            handle.spawn(async move {
                if let Some(new_val) = prompt_text_input(&key, &default, hidden) {
                    let pref_key = PrefKey::new(&key);
                    session.set_preference(&pref_key, toml::Value::String(new_val));
                }
            });
            window.refresh();
        }))
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
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .ok()?;
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

    let (track_bg, knob_offset) = if checked {
        (tc(ctx, |t| t.success), px(18.0))
    } else {
        (gpui::hsla(0.0, 0.0, 1.0, 0.2), px(2.0))
    };

    let track = div()
        .w(px(36.0))
        .h(px(20.0))
        .rounded(px(10.0))
        .bg(track_bg)
        .relative()
        .child(
            div()
                .absolute()
                .top(px(2.0))
                .left(knob_offset)
                .w(px(16.0))
                .h(px(16.0))
                .rounded(px(8.0))
                .bg(gpui::rgba(0xffffffee)),
        );

    let session = ctx.session().clone();
    let key_owned = key.to_string();
    let new_value = !checked;
    let el_id = format!("pref-toggle-{key}");

    div().child(
        div()
            .id(ElementId::Name(el_id.into()))
            .cursor_pointer()
            .child(track)
            .on_mouse_down(gpui::MouseButton::Left, move |_, window, _| {
                let pref_key = PrefKey::new(&key_owned);
                session.set_preference(
                    &pref_key,
                    toml::Value::Boolean(new_value),
                );
                window.refresh();
            }),
    )
}
