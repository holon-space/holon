use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gpui::prelude::*;
use gpui::*;
use gpui_component::input::{
    Enter, Escape, Input, InputEvent, InputState, MoveDown, MoveUp, RopeExt as _,
};
use holon_api::render_types::OperationWiring;
use holon_api::Value;
use holon_frontend::command_menu::{MenuAction, MenuKey, MenuPhase, MenuState};
use holon_frontend::input_trigger::{self, InputTrigger, ViewEvent};
use holon_frontend::view_event_handler::ViewEventHandler;
use holon_frontend::FrontendSession;

use crate::geometry::BoundsRegistry;

/// A persistent GPUI view for an editable text field.
///
/// Owns the `Entity<InputState>`, cursor position, undo history, and command
/// menu state. Created once per editable text node and reused across renders.
pub struct EditorView {
    input: Entity<InputState>,
    handler: Arc<Mutex<ViewEventHandler>>,
    bounds_registry: BoundsRegistry,
}

impl EditorView {
    pub fn new(
        _el_id: String,
        content: String,
        field: String,
        row_id: String,
        operations: Vec<OperationWiring>,
        triggers: Vec<InputTrigger>,
        session: Arc<FrontendSession>,
        rt_handle: tokio::runtime::Handle,
        bounds_registry: BoundsRegistry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .default_value(&content)
        });

        let context_params = HashMap::from([("id".into(), Value::String(row_id.clone()))]);
        let handler = Arc::new(Mutex::new(ViewEventHandler::new(
            operations,
            context_params,
            field,
            content,
        )));

        // Subscribe to blur and change events.
        {
            let handler_clone = handler.clone();
            let session_clone = session.clone();
            let rt_clone = rt_handle.clone();
            let triggers = triggers.clone();
            cx.subscribe_in(
                &input,
                window,
                move |_this, entity, event, _window, cx| match event {
                    InputEvent::Blur => {
                        let new_value = entity.read(cx).value().to_string();
                        let action = handler_clone
                            .lock()
                            .unwrap()
                            .handle(ViewEvent::TextSync { value: new_value });
                        dispatch_menu_action(action, &rt_clone, &session_clone, &handler_clone);
                    }
                    InputEvent::Change => {
                        let text = entity.read(cx).value().to_string();
                        let cursor_pos = entity.read(cx).cursor_position();
                        let cursor_line = cursor_pos.line as usize;
                        let current_line = text.lines().nth(cursor_line).unwrap_or("");
                        let cursor_column = cursor_pos.character as usize;

                        let view_event =
                            input_trigger::check_triggers(&triggers, current_line, cursor_column);

                        let action = if let Some(event) = view_event {
                            handler_clone.lock().unwrap().handle(event)
                        } else {
                            handler_clone
                                .lock()
                                .unwrap()
                                .handle(ViewEvent::TriggerDismissed {
                                    action: "command_menu".to_string(),
                                })
                        };

                        dispatch_menu_action(action, &rt_clone, &session_clone, &handler_clone);
                        cx.notify();
                    }
                    _ => {}
                },
            )
            .detach();
        }

        Self {
            input,
            handler,
            bounds_registry,
        }
    }
}

impl Render for EditorView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let editor_entity_id = self.input.entity_id();
        let menu_overlay = {
            let h = self.handler.lock().unwrap();
            h.command_menu
                .menu_state()
                .map(|state| render_command_menu(state, &self.bounds_registry))
        };

        div()
            .w_full()
            .relative()
            .capture_action({
                let input_state = self.input.clone();
                let handler = self.handler.clone();
                move |_: &MoveUp, _window, cx: &mut App| {
                    let is_active = handler.lock().unwrap().is_overlay_active();
                    if is_active {
                        handler.lock().unwrap().on_key(MenuKey::Up);
                        cx.stop_propagation();
                        cx.notify(editor_entity_id);
                    } else {
                        let pos = input_state.read(cx).cursor_position();
                        if pos.line == 0 {
                            cx.stop_propagation();
                        } else {
                            cx.propagate();
                        }
                    }
                }
            })
            .capture_action({
                let input_state = self.input.clone();
                let handler = self.handler.clone();
                move |_: &MoveDown, _window, cx: &mut App| {
                    let is_active = handler.lock().unwrap().is_overlay_active();
                    if is_active {
                        handler.lock().unwrap().on_key(MenuKey::Down);
                        cx.stop_propagation();
                        cx.notify(editor_entity_id);
                    } else {
                        let on_last_line = {
                            let s = input_state.read(cx);
                            s.cursor_position().line as usize
                                >= s.text().lines_len().saturating_sub(1)
                        };
                        if on_last_line {
                            cx.stop_propagation();
                        } else {
                            cx.propagate();
                        }
                    }
                }
            })
            .capture_action({
                let handler = self.handler.clone();
                move |_: &Enter, _window, cx: &mut App| {
                    let is_active = handler.lock().unwrap().is_overlay_active();
                    if is_active {
                        handler.lock().unwrap().on_key(MenuKey::Enter);
                        cx.stop_propagation();
                        cx.notify(editor_entity_id);
                    } else {
                        cx.propagate();
                    }
                }
            })
            .capture_action({
                let handler = self.handler.clone();
                move |_: &Escape, _window, cx: &mut App| {
                    let is_active = handler.lock().unwrap().is_overlay_active();
                    if is_active {
                        handler.lock().unwrap().on_key(MenuKey::Escape);
                        cx.stop_propagation();
                        cx.notify(editor_entity_id);
                    } else {
                        cx.propagate();
                    }
                }
            })
            .child(Input::new(&self.input).appearance(false))
            .when_some(menu_overlay, |d, overlay| d.child(overlay))
    }
}

fn render_command_menu(state: &MenuState, registry: &BoundsRegistry) -> gpui::Div {
    use gpui::prelude::*;
    use gpui::{div, px};

    let theme = registry.theme();
    let bg = theme.popover;
    let border = theme.border;
    let text_color = theme.foreground;
    let selected_bg = theme.accent;
    let selected_text = theme.accent_foreground;
    let muted = theme.muted_foreground;

    let mut container = div()
        .absolute()
        .left_0()
        .top_full()
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

    match &state.phase {
        MenuPhase::CommandList => {
            if state.matches.is_empty() {
                container = container.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_color(muted)
                        .child("No matching commands"),
                );
            } else {
                for (i, matched) in state.matches.iter().enumerate() {
                    let is_selected = i == state.selected_index;
                    let item = div()
                        .px_2()
                        .py_1()
                        .rounded(px(4.0))
                        .when(is_selected, |d| d.bg(selected_bg).text_color(selected_text))
                        .child(matched.descriptor.display_name.clone());
                    container = container.child(item);
                }
            }
        }
        MenuPhase::ParamCollection {
            operation,
            param,
            search_results,
            selected_index,
            ..
        } => {
            container = container.child(div().px_2().py_1().text_color(muted).text_xs().child(
                format!(
                    "{}: select {}",
                    operation.descriptor.display_name, param.name
                ),
            ));
            if search_results.is_empty() {
                container = container.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_color(muted)
                        .child("Type to search..."),
                );
            } else {
                for (i, row) in search_results.iter().enumerate() {
                    let is_selected = i == *selected_index;
                    let label = row
                        .get("content")
                        .and_then(|v| v.as_string())
                        .unwrap_or("(untitled)")
                        .to_string();
                    let item = div()
                        .px_2()
                        .py_1()
                        .rounded(px(4.0))
                        .when(is_selected, |d| d.bg(selected_bg).text_color(selected_text))
                        .child(label);
                    container = container.child(item);
                }
            }
        }
    }

    container
}

fn dispatch_menu_action(
    action: MenuAction,
    handle: &tokio::runtime::Handle,
    session: &Arc<FrontendSession>,
    handler: &Arc<Mutex<ViewEventHandler>>,
) {
    match action {
        MenuAction::Execute {
            entity_name,
            op_name,
            params,
        } => {
            holon_frontend::operations::dispatch_operation(
                handle,
                session,
                entity_name,
                op_name,
                params,
            );
        }
        MenuAction::SearchEntities { query, .. } => {
            let session = session.clone();
            let handler = handler.clone();
            handle.spawn(async move {
                let sql = format!(
                    "SELECT id, content FROM block WHERE content LIKE '%{}%' LIMIT 20",
                    query.replace('\'', "''")
                );
                if let Ok(results) = session
                    .engine()
                    .execute_query(sql, HashMap::new(), None)
                    .await
                {
                    handler
                        .lock()
                        .unwrap()
                        .command_menu
                        .set_search_results(results);
                }
            });
        }
        MenuAction::Updated | MenuAction::Dismissed | MenuAction::NotActive => {}
    }
}
