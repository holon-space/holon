use std::sync::{Arc, Mutex};

use gpui::prelude::*;
use gpui::*;
use gpui_component::input::{
    Enter, Escape, IndentInline, Input, InputEvent, InputState, MoveDown, MoveUp, OutdentInline,
};
use holon_frontend::editor_controller::{EditorAction, EditorController, EditorKey};
use holon_frontend::popup_menu::PopupState;
use holon_frontend::reactive::BuilderServices;

use gpui_component::RopeExt;

/// A persistent GPUI view for an editable text field.
///
/// Thin wrapper around `EditorController` (framework-agnostic logic).
/// GPUI-specific responsibilities: InputState entity, GPUI action capture,
/// popup overlay rendering, signal watching, cursor manipulation.
pub struct EditorView {
    input: Entity<InputState>,
    controller: Arc<Mutex<EditorController>>,
    row_id: String,
    services: Arc<dyn BuilderServices>,
}

impl EditorView {
    pub fn new(
        _el_id: String,
        content: String,
        field: String,
        row_id: String,
        operations: Vec<holon_api::render_types::OperationWiring>,
        triggers: Vec<holon_frontend::input_trigger::InputTrigger>,
        services: Arc<dyn BuilderServices>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .auto_grow(1, usize::MAX)
                .default_value(&content)
        });

        let context_params = std::collections::HashMap::from([(
            "id".into(),
            holon_api::Value::String(row_id.clone()),
        )]);
        let mut controller =
            EditorController::new(operations, triggers, context_params, field, content);
        controller.set_async_context(services.clone());
        let controller = Arc::new(Mutex::new(controller));

        // Subscribe to blur and change events.
        {
            let ctrl = controller.clone();
            let services_clone = services.clone();
            let row_id_for_blur = row_id.clone();
            cx.subscribe_in(
                &input,
                window,
                move |this, entity, event, _window, cx| match event {
                    InputEvent::Blur => {
                        let value = entity.read(cx).value().to_string();
                        let action = ctrl.lock().unwrap().on_blur(&value);
                        execute_action(action, &services_clone, this.input.entity_id(), cx);

                        // Persist cursor position on blur — but only if focus
                        // is still on this block. During cross-block arrow-key
                        // navigation, set_focus() already moved to the new
                        // block before on_blur fires. Persisting the OLD
                        // block's cursor would trigger watch_editor_cursor →
                        // window.focus(old) → stealing focus back.
                        let my_uri = holon_api::EntityUri::from_raw(&row_id_for_blur);
                        let still_mine = services_clone.focused_block().as_ref() == Some(&my_uri);
                        if still_mine {
                            let cursor_byte = entity.read(cx).cursor();
                            let mut params = std::collections::HashMap::new();
                            params.insert("region".into(), holon_api::Value::String("main".into()));
                            params.insert(
                                "block_id".into(),
                                holon_api::Value::String(row_id_for_blur.clone()),
                            );
                            params.insert(
                                "cursor_offset".into(),
                                holon_api::Value::Integer(cursor_byte as i64),
                            );
                            services_clone.dispatch_intent(holon_frontend::OperationIntent::new(
                                "navigation".into(),
                                "editor_focus".into(),
                                params,
                            ));
                        }
                    }
                    InputEvent::Change => {
                        let text = entity.read(cx).value().to_string();
                        let cursor_pos = entity.read(cx).cursor_position();
                        let cursor_line = cursor_pos.line as usize;
                        let current_line = text.lines().nth(cursor_line).unwrap_or("");
                        let cursor_column = cursor_pos.character as usize;

                        let action = ctrl
                            .lock()
                            .unwrap()
                            .on_text_changed(current_line, cursor_column);
                        execute_action(action, &services_clone, this.input.entity_id(), cx);
                        cx.notify();
                    }
                    _ => {}
                },
            )
            .detach();
        }

        Self {
            input,
            controller,
            row_id,
            services,
        }
    }
}

impl EditorView {
    pub fn row_id(&self) -> &str {
        &self.row_id
    }

    pub fn input_entity(&self) -> &Entity<InputState> {
        &self.input
    }
}

impl Render for EditorView {
    #[tracing::instrument(
        level = "trace",
        skip_all,
        name = "frontend.render",
        fields(component = "editor")
    )]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let editor_entity_id = self.input.entity_id();
        let popup_overlay = {
            let ctrl = self.controller.lock().unwrap();
            ctrl.popup_state().map(|s| render_popup(&s, cx))
        };

        let window_handle = window.window_handle();

        div()
            .w_full()
            .relative()
            .capture_action({
                let ctrl = self.controller.clone();
                move |_: &MoveUp, _window, cx: &mut App| {
                    let action = ctrl.lock().unwrap().on_key(EditorKey::Up);
                    if !matches!(action, EditorAction::Propagate | EditorAction::None) {
                        cx.stop_propagation();
                        cx.notify(editor_entity_id);
                    }
                }
            })
            .capture_action({
                let ctrl = self.controller.clone();
                move |_: &MoveDown, _window, cx: &mut App| {
                    let action = ctrl.lock().unwrap().on_key(EditorKey::Down);
                    if !matches!(action, EditorAction::Propagate | EditorAction::None) {
                        cx.stop_propagation();
                        cx.notify(editor_entity_id);
                    }
                }
            })
            .capture_action({
                let ctrl = self.controller.clone();
                let input = self.input.clone();
                let services = self.services.clone();
                let row_id = self.row_id.clone();
                move |_: &Enter, window, cx: &mut App| {
                    // Cmd+Enter → dispatch cycle_task_state.
                    // GPUI's action system captures Enter before on_key_down fires,
                    // so we handle the keychord here directly.
                    if window.modifiers().platform {
                        let mut params = std::collections::HashMap::new();
                        params.insert("id".into(), holon_api::Value::String(row_id.clone()));
                        services.dispatch_intent(holon_frontend::operations::OperationIntent::new(
                            "block".into(),
                            "cycle_task_state".into(),
                            params,
                        ));
                        cx.stop_propagation();
                        return;
                    }
                    let action = ctrl.lock().unwrap().on_key(EditorKey::Enter);
                    match action {
                        EditorAction::InsertText {
                            replacement,
                            prefix_start,
                        } => {
                            let text = input.read(cx).value().to_string();
                            let cursor = input.read(cx).cursor();
                            let cursor_pos = input.read(cx).cursor_position();
                            let line_start = cursor - cursor_pos.character as usize;
                            let abs_start = line_start + prefix_start;

                            let mut new_text =
                                String::with_capacity(text.len() + replacement.len());
                            new_text.push_str(&text[..abs_start]);
                            new_text.push_str(&replacement);
                            new_text.push_str(&text[cursor..]);
                            let new_cursor_offset = abs_start + replacement.len();

                            let input = input.clone();
                            cx.spawn(async move |cx| {
                                let _ = cx.update_window(window_handle, |_, window, cx| {
                                    input.update(cx, |state, cx| {
                                        state.set_value(&new_text, window, cx);
                                        let pos =
                                            state.text().offset_to_position(new_cursor_offset);
                                        state.set_cursor_position(pos, window, cx);
                                    });
                                });
                            })
                            .detach();
                            cx.stop_propagation();
                            cx.notify(editor_entity_id);
                        }
                        EditorAction::Execute(intent) => {
                            services.dispatch_intent(intent);
                            cx.stop_propagation();
                            cx.notify(editor_entity_id);
                        }
                        EditorAction::PopupDismissed | EditorAction::UpdatePopup => {
                            cx.stop_propagation();
                            cx.notify(editor_entity_id);
                        }
                        EditorAction::None => {
                            // No popup active → propagate Enter to parent.
                            // The block row handles it via the operation dispatch system.
                        }
                        EditorAction::Propagate => {
                            cx.propagate();
                        }
                        EditorAction::PopupActivated { .. } => {
                            // Enter shouldn't activate a popup, but handle gracefully
                            cx.stop_propagation();
                            cx.notify(editor_entity_id);
                        }
                    }
                }
            })
            .capture_action({
                let ctrl = self.controller.clone();
                move |_: &Escape, _window, cx: &mut App| {
                    let action = ctrl.lock().unwrap().on_key(EditorKey::Escape);
                    if !matches!(action, EditorAction::Propagate | EditorAction::None) {
                        cx.stop_propagation();
                        cx.notify(editor_entity_id);
                    }
                }
            })
            // Intercept Tab/Shift+Tab before InputState consumes them for
            // tab-character insertion. Dispatch indent/outdent directly,
            // matching the Enter → split_block pattern above.
            .capture_action({
                let services = self.services.clone();
                let row_id = self.row_id.clone();
                move |_: &IndentInline, _window, cx: &mut App| {
                    let mut params = std::collections::HashMap::new();
                    params.insert("id".into(), holon_api::Value::String(row_id.clone()));
                    services.dispatch_intent(holon_frontend::operations::OperationIntent::new(
                        "block".into(),
                        "indent".into(),
                        params,
                    ));
                    cx.stop_propagation();
                }
            })
            .capture_action({
                let services = self.services.clone();
                let row_id = self.row_id.clone();
                move |_: &OutdentInline, _window, cx: &mut App| {
                    let mut params = std::collections::HashMap::new();
                    params.insert("id".into(), holon_api::Value::String(row_id.clone()));
                    services.dispatch_intent(holon_frontend::operations::OperationIntent::new(
                        "block".into(),
                        "outdent".into(),
                        params,
                    ));
                    cx.stop_propagation();
                }
            })
            .child(Input::new(&self.input).appearance(false))
            .when_some(popup_overlay, |d, overlay| d.child(overlay))
    }
}

/// Render the unified popup overlay.
fn render_popup(state: &PopupState, cx: &App) -> Deferred {
    use gpui::prelude::*;
    use gpui::{div, px};
    use gpui_component::theme::ActiveTheme;

    let theme = cx.theme().colors;
    let bg = theme.popover;
    let border = theme.border;
    let text_color = theme.foreground;
    let selected_bg = theme.accent;
    let selected_text = theme.accent_foreground;
    let muted = theme.muted_foreground;

    let mut container = div()
        .absolute()
        .left_0()
        .top(px(20.0))
        .w(px(280.0))
        .max_h(px(240.0))
        .overflow_y_hidden()
        .bg(bg)
        .border_1()
        .border_color(border)
        .rounded(px(6.0))
        .shadow_md()
        .p_1()
        .flex_col()
        .text_color(text_color)
        .text_sm();

    if state.items.is_empty() {
        container = container.child(
            div()
                .px_2()
                .py_1()
                .text_color(muted)
                .child("Type to search..."),
        );
    } else {
        for (i, item) in state.items.iter().enumerate() {
            let is_selected = i == state.selected_index;
            let mut row = div()
                .px_2()
                .py_1()
                .rounded(px(4.0))
                .when(is_selected, |d| d.bg(selected_bg).text_color(selected_text));

            if let Some(icon) = &item.icon {
                row = row.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(icon.clone())
                        .child(item.label.clone()),
                );
            } else {
                row = row.child(item.label.clone());
            }
            container = container.child(row);
        }
    }

    deferred(container).with_priority(1)
}

/// Execute an EditorAction in a context without window access (subscribe callbacks).
fn execute_action<T: 'static>(
    action: EditorAction,
    services: &Arc<dyn BuilderServices>,
    editor_entity_id: EntityId,
    cx: &mut Context<T>,
) {
    match action {
        EditorAction::PopupActivated { signal } => {
            cx.spawn(async move |_this, cx| {
                use futures_signals::signal::SignalExt;
                signal
                    .for_each(|_items| {
                        let _ = cx.update(|cx| {
                            cx.notify(editor_entity_id);
                        });
                        async {}
                    })
                    .await;
            })
            .detach();
        }
        EditorAction::Execute(intent) => {
            services.dispatch_intent(intent);
        }
        // UpdatePopup, Dismissed, InsertText, None, Propagate — no action needed
        // in the no-window context (subscribe callbacks). The caller handles cx.notify().
        _ => {}
    }
}
