use std::collections::HashMap;

use holon_api::render_types::OperationWiring;
use holon_api::Value;

use crate::command_menu::{CommandMenuController, MenuAction, MenuKey};
use crate::input_trigger::ViewEvent;
use crate::operations::find_set_field_op;

/// Handles ViewEvents for an editable text node.
///
/// Each editable text node gets one of these. It routes ViewEvents to the
/// appropriate handler (command menu, doc links, mentions, etc.) and returns
/// actions for the frontend to execute.
pub struct ViewEventHandler {
    pub command_menu: CommandMenuController,
    /// The field this editable text node is editing (e.g., "content").
    field: String,
    /// The original text value when the handler was created. Used to detect
    /// whether TextSync actually changed anything.
    original_value: String,
    /// Pre-resolved operation metadata for the set_field path.
    set_field_entity: Option<String>,
    set_field_op: Option<String>,
    /// Context params (includes row id, etc.)
    context_params: HashMap<String, Value>,
}

impl ViewEventHandler {
    pub fn new(
        operations: Vec<OperationWiring>,
        context_params: HashMap<String, Value>,
        field: String,
        original_value: String,
    ) -> Self {
        let op = find_set_field_op(&field, &operations);
        let set_field_entity = op.map(|o| o.entity_name.to_string());
        let set_field_op = op.map(|o| o.name.clone());

        Self {
            command_menu: CommandMenuController::new(operations, context_params.clone()),
            field,
            original_value,
            set_field_entity,
            set_field_op,
            context_params,
        }
    }

    /// Process a ViewEvent from the frontend's trigger check.
    /// Returns a MenuAction telling the frontend what to do.
    pub fn handle(&mut self, event: ViewEvent) -> MenuAction {
        match event {
            ViewEvent::TriggerFired {
                action,
                current_line,
                ..
            } => match action.as_str() {
                "command_menu" => self.command_menu.on_text_changed(&current_line),
                _ => MenuAction::NotActive,
            },

            ViewEvent::TriggerDismissed { action } => match action.as_str() {
                "command_menu" => {
                    if self.command_menu.is_active() {
                        self.command_menu.dismiss();
                        MenuAction::Dismissed
                    } else {
                        MenuAction::NotActive
                    }
                }
                _ => MenuAction::NotActive,
            },

            ViewEvent::TextSync { value } => self.handle_text_sync(value),
        }
    }

    /// Handle Tier 3 text sync (blur). If the value changed and we have a
    /// set_field operation, return Execute with the appropriate params.
    fn handle_text_sync(&mut self, new_value: String) -> MenuAction {
        if new_value == self.original_value {
            return MenuAction::NotActive;
        }
        self.original_value = new_value.clone();

        let (Some(entity_name), Some(op_name)) = (&self.set_field_entity, &self.set_field_op)
        else {
            return MenuAction::NotActive;
        };

        let id = self
            .context_params
            .get("id")
            .and_then(|v| v.as_string())
            .expect("ViewEventHandler context_params missing 'id'")
            .to_string();

        let mut params = HashMap::new();
        params.insert("id".into(), Value::String(id));
        params.insert("field".into(), Value::String(self.field.clone()));
        params.insert("value".into(), Value::String(new_value));

        MenuAction::Execute {
            entity_name: entity_name.clone(),
            op_name: op_name.clone(),
            params,
        }
    }

    /// Forward keyboard events to the active handler (if any).
    pub fn on_key(&mut self, key: MenuKey) -> MenuAction {
        if self.command_menu.is_active() {
            return self.command_menu.on_key(key);
        }
        MenuAction::NotActive
    }

    /// Whether any overlay (command menu, autocomplete, etc.) is currently active.
    pub fn is_overlay_active(&self) -> bool {
        self.command_menu.is_active()
    }
}
