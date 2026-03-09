use std::sync::{Arc, Mutex};

use futures_signals::signal::{ReadOnlyMutable, SignalExt};
use gpui::prelude::*;
use gpui::*;
use gpui_component::input::{
    Enter, Escape, IndentInline, Input, InputEvent, InputState, MoveDown, MoveUp, OutdentInline,
    Paste,
};
use gpui_component::menu::PopupMenuItem;
use holon_api::widget_spec::DataRow;
use holon_frontend::editor_controller::{EditorAction, EditorController, EditorKey};
use holon_frontend::popup_menu::PopupState;
use holon_frontend::reactive::BuilderServices;

use crate::share_ui::ShareTrigger;

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
    /// Cancelled on drop (GPUI `Task` semantics). Owns the data →
    /// InputState propagation task that keeps the editor in sync with
    /// external row updates (peer edits, file reloads, split_block
    /// truncations) without polling on every render. The render path no
    /// longer touches `set_value`.
    _data_subscription: Option<Task<()>>,
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
        // Shared per-row data cell from `ReactiveRowSet`. When `Some`, the
        // editor subscribes to it and keeps `InputState` in sync with
        // backend updates. When `None` (snapshot/test paths), the editor
        // shows the initial `content` and never updates from data.
        data: Option<ReadOnlyMutable<Arc<DataRow>>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            let row_id_for_menu = row_id.clone();
            InputState::new(window, cx)
                .auto_grow(1, usize::MAX)
                .default_value(&content)
                .context_menu_extender(move |menu, _window, _cx| {
                    let row_id_for_click = row_id_for_menu.clone();
                    menu.separator()
                        .item(PopupMenuItem::new("Share subtree…").on_click(
                            move |_, _window, cx| {
                                ShareTrigger::trigger(row_id_for_click.clone(), cx);
                            },
                        ))
                })
        });

        let context_params = std::collections::HashMap::from([(
            "id".into(),
            holon_api::Value::String(row_id.clone()),
        )]);
        let field_for_subscription = field.clone();
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
                    InputEvent::Focus => {
                        #[cfg(feature = "mobile")]
                        gpui_mobile::show_keyboard();
                    }
                    InputEvent::Blur => {
                        #[cfg(feature = "mobile")]
                        gpui_mobile::hide_keyboard();

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

        // Data → InputState propagation. Subscribes to the shared per-row
        // signal cell from `ReactiveRowSet` and applies external row
        // changes (peer edits, file reloads, split_block truncations,
        // CDC echoes of our own writes) into the local InputState.
        //
        // Two safeguards:
        //
        // 1. **Skip when focused.** While the user has the editor focused
        //    they are the source of truth — overwriting `InputState`
        //    while they're typing yanks the cursor to position 0 and
        //    drops the in-flight character. External changes during a
        //    focused edit are dropped from the *visible* state until the
        //    next focus cycle (data is still correct in the backend).
        //
        // 2. **Dedupe on the field's value.** The signal fires on every
        //    `.set()` of the per-row Mutable, including no-op writes
        //    triggered by unrelated field changes. `.dedupe_cloned()` on
        //    the extracted field value keeps the subscription quiet
        //    unless the relevant column actually changed.
        //
        // The render path no longer touches `set_value` — propagation is
        // entirely event-driven through this subscription. The returned
        // `Task<()>` cancels on drop, so removing this `EditorView`
        // (e.g. via collection driver `RemoveAt`) tears the subscription
        // down naturally.
        let _data_subscription: Option<Task<()>> = data.map(|data_handle| {
            let field_for_stream = field_for_subscription;
            let signal = data_handle
                .signal_cloned()
                .map(move |row| {
                    row.get(&field_for_stream)
                        .and_then(|v| v.as_string())
                        .unwrap_or("")
                        .to_string()
                })
                .dedupe_cloned();
            cx.spawn(async move |this, cx| {
                use futures::StreamExt;
                let mut stream = signal.to_stream();
                // Drop the initial emission — it just re-states the
                // `default_value` we already passed to `InputState::new`.
                let _ = stream.next().await;
                while let Some(new_value) = stream.next().await {
                    if this.upgrade().is_none() {
                        // EditorView dropped (e.g. row removed by
                        // collection driver). Stop the loop — the `Task`
                        // will be dropped shortly when our owning struct
                        // is freed, but exiting cleanly avoids a tight
                        // spin while the Drop runs.
                        break;
                    }
                    cx.update(|cx| {
                        let Some(view) = this.upgrade() else {
                            return;
                        };
                        let input = view.read(cx).input.clone();
                        // Focus is window-scoped; pick the first window
                        // that owns this input entity. There is exactly
                        // one in normal app usage.
                        for window_handle in cx.windows() {
                            let _ = window_handle.update(cx, |_, window, cx| {
                                let focused = input.read(cx).focus_handle(cx).is_focused(window);
                                if focused {
                                    return;
                                }
                                input.update(cx, |state, cx| {
                                    if state.value().to_string() != new_value {
                                        state.set_value(&new_value, window, cx);
                                    }
                                });
                            });
                        }
                    });
                }
            })
        });

        Self {
            input,
            controller,
            row_id,
            services,
            _data_subscription,
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
                            // No popup active → split the block at the cursor.
                            // We can't rely on Enter bubbling to lib.rs's chord
                            // resolver: gpui-component's InputState consumes
                            // Enter for multi-line newline insertion (auto_grow
                            // sets max_rows > 1, making is_multi_line() true),
                            // so the bubble-phase on_action never fires.
                            // Dispatch split_block directly, matching the
                            // Tab → indent / Shift+Tab → outdent pattern below.
                            let cursor_byte = input.read(cx).cursor();
                            let mut params = std::collections::HashMap::new();
                            params.insert("id".into(), holon_api::Value::String(row_id.clone()));
                            params.insert(
                                "position".into(),
                                holon_api::Value::Integer(cursor_byte as i64),
                            );
                            services.dispatch_intent(
                                holon_frontend::operations::OperationIntent::new(
                                    "block".into(),
                                    "split_block".into(),
                                    params,
                                ),
                            );
                            cx.stop_propagation();
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
            .capture_action({
                let services = self.services.clone();
                let row_id = self.row_id.clone();
                move |_: &Paste, _window, cx: &mut App| {
                    if let Some(clipboard) = cx.read_from_clipboard() {
                        for entry in clipboard.entries() {
                            if let ClipboardEntry::Image(image) = entry {
                                let ext = match image.format {
                                    ImageFormat::Png => "png",
                                    ImageFormat::Jpeg => "jpeg",
                                    ImageFormat::Gif => "gif",
                                    ImageFormat::Webp => "webp",
                                    ImageFormat::Svg => "svg",
                                    ImageFormat::Bmp => "bmp",
                                    ImageFormat::Tiff => "tiff",
                                    ImageFormat::Ico => "ico",
                                };
                                match save_clipboard_image(&image.bytes, ext) {
                                    Ok(relative_path) => {
                                        let new_id = holon_api::EntityUri::block_random();
                                        let mut params = std::collections::HashMap::new();
                                        params.insert(
                                            "id".into(),
                                            holon_api::Value::String(new_id.to_string()),
                                        );
                                        params.insert(
                                            "content".into(),
                                            holon_api::Value::String(relative_path),
                                        );
                                        params.insert(
                                            "content_type".into(),
                                            holon_api::Value::String("image".into()),
                                        );
                                        params.insert(
                                            "after".into(),
                                            holon_api::Value::String(row_id.clone()),
                                        );
                                        services.dispatch_intent(
                                            holon_frontend::operations::OperationIntent::new(
                                                "block".into(),
                                                "create".into(),
                                                params,
                                            ),
                                        );
                                    }
                                    Err(e) => {
                                        tracing::error!("Failed to save pasted image: {e}");
                                    }
                                }
                                cx.stop_propagation();
                                return;
                            }
                        }
                    }
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

/// Save clipboard image bytes to the org attachments directory.
/// Returns the relative path (e.g. "attachments/a1b2c3d4.png").
fn save_clipboard_image(bytes: &[u8], extension: &str) -> Result<String, std::io::Error> {
    let root = org_root_dir();
    let attachments = root.join("attachments");
    std::fs::create_dir_all(&attachments)?;

    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    let hash = hasher.finish();
    let filename = format!("{hash:016x}.{extension}");
    let abs_path = attachments.join(&filename);

    if !abs_path.exists() {
        std::fs::write(&abs_path, bytes)?;
        tracing::info!("Saved pasted image to {}", abs_path.display());
    }
    Ok(format!("attachments/{filename}"))
}

fn org_root_dir() -> std::path::PathBuf {
    if let Ok(root) = std::env::var("HOLON_ORGMODE_ROOT_DIRECTORY") {
        return std::path::PathBuf::from(root);
    }
    if let Ok(root) = std::env::var("HOLON_WORKSPACE_ROOT") {
        return std::path::PathBuf::from(root);
    }
    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
        return std::path::PathBuf::from(manifest);
    }
    std::path::PathBuf::from(".")
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
