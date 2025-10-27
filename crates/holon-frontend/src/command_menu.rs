use std::collections::HashMap;

use holon_api::render_types::{OperationParam, OperationWiring};
use holon_api::Value;

use crate::operation_matcher::{self, MatchedOperation};

/// Action returned from the command menu controller telling the frontend what to do.
#[derive(Debug)]
pub enum MenuAction {
    /// Menu not relevant (no `/` prefix or no matches).
    NotActive,
    /// Menu state updated, frontend should re-render the menu overlay.
    Updated,
    /// Menu dismissed (Escape or backspace past `/`).
    Dismissed,
    /// Operation fully resolved — dispatch it.
    Execute {
        entity_name: String,
        op_name: String,
        params: HashMap<String, Value>,
    },
    /// Need to search for entities to fill a missing param.
    /// Frontend should query and call `set_search_results()`.
    SearchEntities { entity_name: String, query: String },
}

/// Phase of the slash command menu interaction.
#[derive(Debug, Clone)]
pub enum MenuPhase {
    /// Showing filtered operation list.
    CommandList,
    /// Collecting a missing param — showing entity search results.
    ParamCollection {
        operation: MatchedOperation,
        param: OperationParam,
        search_query: String,
        search_results: Vec<HashMap<String, Value>>,
        selected_index: usize,
    },
}

/// Visible state of the command menu for rendering.
#[derive(Debug, Clone)]
pub struct MenuState {
    pub filter: String,
    pub selected_index: usize,
    pub matches: Vec<MatchedOperation>,
    pub phase: MenuPhase,
}

/// Frontend-agnostic slash command menu controller.
///
/// Each editable_text widget creates one of these. Drive it with text changes
/// and key events; it returns `MenuAction`s telling the frontend what to render
/// or dispatch.
pub struct CommandMenuController {
    operations: Vec<OperationWiring>,
    state: Option<MenuState>,
    /// Available params from the editing context (entity id, content, etc.)
    context_params: HashMap<String, Value>,
}

impl CommandMenuController {
    pub fn new(operations: Vec<OperationWiring>, context_params: HashMap<String, Value>) -> Self {
        Self {
            operations,
            state: None,
            context_params,
        }
    }

    /// Whether the menu is currently active.
    pub fn is_active(&self) -> bool {
        self.state.is_some()
    }

    /// Get the current menu state for rendering (if active).
    pub fn menu_state(&self) -> Option<&MenuState> {
        self.state.as_ref()
    }

    /// Explicitly dismiss the menu.
    pub fn dismiss(&mut self) {
        self.state = None;
    }

    /// Called when the text in the editable field changes.
    /// Detects `/` at the start of the current line and filters operations.
    pub fn on_text_changed(&mut self, current_line: &str) -> MenuAction {
        if !current_line.starts_with('/') {
            if self.state.is_some() {
                self.state = None;
                return MenuAction::Dismissed;
            }
            return MenuAction::NotActive;
        }

        let filter = &current_line[1..]; // text after `/`

        // If we're in param collection phase, route to search
        if let Some(state) = &self.state {
            if let MenuPhase::ParamCollection { .. } = &state.phase {
                return self.update_param_search(filter);
            }
        }

        // Filter operations by name
        let all_matches =
            operation_matcher::find_satisfiable(&self.operations, &self.context_params);

        let filtered: Vec<MatchedOperation> = if filter.is_empty() {
            all_matches
        } else {
            let filter_lower = filter.to_lowercase();
            all_matches
                .into_iter()
                .filter(|m| {
                    m.descriptor.name.to_lowercase().contains(&filter_lower)
                        || m.descriptor
                            .display_name
                            .to_lowercase()
                            .contains(&filter_lower)
                })
                .collect()
        };

        self.state = Some(MenuState {
            filter: filter.to_string(),
            selected_index: 0,
            matches: filtered,
            phase: MenuPhase::CommandList,
        });

        MenuAction::Updated
    }

    /// Handle keyboard input while menu is open.
    pub fn on_key(&mut self, key: MenuKey) -> MenuAction {
        let state = match &mut self.state {
            Some(s) => s,
            None => return MenuAction::NotActive,
        };

        match (&mut state.phase, key) {
            (MenuPhase::CommandList, MenuKey::Up) => {
                if state.selected_index > 0 {
                    state.selected_index -= 1;
                }
                MenuAction::Updated
            }
            (MenuPhase::CommandList, MenuKey::Down) => {
                if state.selected_index + 1 < state.matches.len() {
                    state.selected_index += 1;
                }
                MenuAction::Updated
            }
            (MenuPhase::CommandList, MenuKey::Enter) => self.select_current(),
            (MenuPhase::CommandList, MenuKey::Escape) => {
                self.state = None;
                MenuAction::Dismissed
            }
            (MenuPhase::ParamCollection { .. }, MenuKey::Up) => {
                if let MenuPhase::ParamCollection { selected_index, .. } = &mut state.phase {
                    if *selected_index > 0 {
                        *selected_index -= 1;
                    }
                }
                MenuAction::Updated
            }
            (MenuPhase::ParamCollection { .. }, MenuKey::Down) => {
                if let MenuPhase::ParamCollection {
                    selected_index,
                    search_results,
                    ..
                } = &mut state.phase
                {
                    if *selected_index + 1 < search_results.len() {
                        *selected_index += 1;
                    }
                }
                MenuAction::Updated
            }
            (MenuPhase::ParamCollection { .. }, MenuKey::Enter) => self.select_search_result(),
            (MenuPhase::ParamCollection { .. }, MenuKey::Escape) => {
                // Go back to command list
                state.phase = MenuPhase::CommandList;
                MenuAction::Updated
            }
            (_, MenuKey::Tab) => {
                // Tab acts like Enter
                self.on_key(MenuKey::Enter)
            }
        }
    }

    /// Provide search results for entity param collection.
    pub fn set_search_results(&mut self, results: Vec<HashMap<String, Value>>) {
        if let Some(state) = &mut self.state {
            if let MenuPhase::ParamCollection {
                search_results,
                selected_index,
                ..
            } = &mut state.phase
            {
                *search_results = results;
                *selected_index = 0;
            }
        }
    }

    fn select_current(&mut self) -> MenuAction {
        let state = match &mut self.state {
            Some(s) => s,
            None => return MenuAction::NotActive,
        };

        let idx = state.selected_index;
        if idx >= state.matches.len() {
            return MenuAction::NotActive;
        }

        let matched = state.matches[idx].clone();

        if matched.is_fully_satisfied() {
            self.state = None;
            return MenuAction::Execute {
                entity_name: matched.entity_name().to_string(),
                op_name: matched.operation_name().to_string(),
                params: matched.resolved_params,
            };
        }

        // Has missing params — check if any need entity search
        let entity_params = matched.entity_params_needed();
        if let Some(&(_, entity_name)) = entity_params.first() {
            let first_missing = matched.missing_params[0].clone();
            let entity_name_str = entity_name.to_string();
            state.phase = MenuPhase::ParamCollection {
                operation: matched,
                param: first_missing,
                search_query: String::new(),
                search_results: vec![],
                selected_index: 0,
            };
            return MenuAction::SearchEntities {
                entity_name: entity_name_str,
                query: String::new(),
            };
        }

        // Missing params but no entity search — can't auto-collect
        MenuAction::NotActive
    }

    fn update_param_search(&mut self, text: &str) -> MenuAction {
        let state = match &mut self.state {
            Some(s) => s,
            None => return MenuAction::NotActive,
        };

        if let MenuPhase::ParamCollection {
            search_query,
            param,
            ..
        } = &mut state.phase
        {
            *search_query = text.to_string();
            let entity_name = match &param.type_hint {
                holon_api::render_types::TypeHint::EntityId { entity_name } => {
                    entity_name.to_string()
                }
                _ => return MenuAction::Updated,
            };
            return MenuAction::SearchEntities {
                entity_name,
                query: text.to_string(),
            };
        }

        MenuAction::Updated
    }

    fn select_search_result(&mut self) -> MenuAction {
        let state = match &mut self.state {
            Some(s) => s,
            None => return MenuAction::NotActive,
        };

        if let MenuPhase::ParamCollection {
            operation,
            param,
            search_results,
            selected_index,
            ..
        } = &state.phase
        {
            if *selected_index >= search_results.len() {
                return MenuAction::NotActive;
            }

            let result_row = &search_results[*selected_index];
            let selected_id = result_row
                .get("id")
                .and_then(|v| v.as_string())
                .unwrap_or("")
                .to_string();

            let mut params = operation.resolved_params.clone();
            params.insert(param.name.clone(), Value::String(selected_id));

            let entity_name = operation.entity_name().to_string();
            let op_name = operation.operation_name().to_string();

            self.state = None;
            return MenuAction::Execute {
                entity_name,
                op_name,
                params,
            };
        }

        MenuAction::NotActive
    }
}

/// Keyboard keys the command menu handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKey {
    Up,
    Down,
    Enter,
    Escape,
    Tab,
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_api::render_types::{
        OperationDescriptor, OperationParam, OperationWiring, TypeHint, WidgetType,
    };
    use holon_api::types::EntityName;

    fn make_op(name: &str, display: &str, params: Vec<OperationParam>) -> OperationWiring {
        OperationWiring {
            widget_type: WidgetType::Button,
            modified_param: String::new(),
            descriptor: OperationDescriptor {
                entity_name: EntityName::new("block"),
                entity_short_name: "block".into(),
                id_column: "id".into(),
                name: name.into(),
                display_name: display.into(),
                description: String::new(),
                required_params: params,
                affected_fields: vec![],
                param_mappings: vec![],
                precondition: None,
            },
        }
    }

    fn param(name: &str, hint: TypeHint) -> OperationParam {
        OperationParam {
            name: name.into(),
            type_hint: hint,
            description: String::new(),
        }
    }

    fn test_ops() -> Vec<OperationWiring> {
        vec![
            make_op(
                "set_field",
                "Set Field",
                vec![
                    param("id", TypeHint::String),
                    param("field", TypeHint::String),
                    param("value", TypeHint::String),
                ],
            ),
            make_op(
                "embed_entity",
                "Embed",
                vec![
                    param("id", TypeHint::String),
                    param(
                        "target_uri",
                        TypeHint::EntityId {
                            entity_name: EntityName::new("block"),
                        },
                    ),
                ],
            ),
            make_op("delete", "Delete", vec![param("id", TypeHint::String)]),
        ]
    }

    fn context() -> HashMap<String, Value> {
        HashMap::from([("id".into(), Value::String("block-1".into()))])
    }

    #[test]
    fn slash_activates_menu() {
        let mut ctrl = CommandMenuController::new(test_ops(), context());
        let action = ctrl.on_text_changed("/");
        assert!(matches!(action, MenuAction::Updated));
        assert!(ctrl.is_active());

        let state = ctrl.menu_state().unwrap();
        assert!(state.filter.is_empty());
        // Should have matches (delete and embed_entity have id resolved)
        assert!(!state.matches.is_empty());
    }

    #[test]
    fn filter_narrows_results() {
        let mut ctrl = CommandMenuController::new(test_ops(), context());
        ctrl.on_text_changed("/");
        let action = ctrl.on_text_changed("/emb");
        assert!(matches!(action, MenuAction::Updated));

        let state = ctrl.menu_state().unwrap();
        assert_eq!(state.matches.len(), 1);
        assert_eq!(state.matches[0].operation_name(), "embed_entity");
    }

    #[test]
    fn no_slash_dismisses() {
        let mut ctrl = CommandMenuController::new(test_ops(), context());
        ctrl.on_text_changed("/");
        assert!(ctrl.is_active());

        let action = ctrl.on_text_changed("hello");
        assert!(matches!(action, MenuAction::Dismissed));
        assert!(!ctrl.is_active());
    }

    #[test]
    fn escape_dismisses() {
        let mut ctrl = CommandMenuController::new(test_ops(), context());
        ctrl.on_text_changed("/");
        let action = ctrl.on_key(MenuKey::Escape);
        assert!(matches!(action, MenuAction::Dismissed));
        assert!(!ctrl.is_active());
    }

    #[test]
    fn select_fully_satisfied_executes() {
        let mut ctrl = CommandMenuController::new(test_ops(), context());
        ctrl.on_text_changed("/del");

        let state = ctrl.menu_state().unwrap();
        assert_eq!(state.matches.len(), 1);
        assert_eq!(state.matches[0].operation_name(), "delete");
        assert!(state.matches[0].is_fully_satisfied());

        let action = ctrl.on_key(MenuKey::Enter);
        assert!(matches!(action, MenuAction::Execute { .. }));
        if let MenuAction::Execute {
            op_name, params, ..
        } = action
        {
            assert_eq!(op_name, "delete");
            assert_eq!(params["id"], Value::String("block-1".into()));
        }
    }

    #[test]
    fn select_with_missing_entity_param_triggers_search() {
        let mut ctrl = CommandMenuController::new(test_ops(), context());
        ctrl.on_text_changed("/emb");
        let action = ctrl.on_key(MenuKey::Enter);

        assert!(matches!(action, MenuAction::SearchEntities { .. }));
        if let MenuAction::SearchEntities { entity_name, .. } = action {
            assert_eq!(entity_name, "block");
        }

        // Provide search results
        ctrl.set_search_results(vec![HashMap::from([(
            "id".into(),
            Value::String("target-block".into()),
        )])]);

        // Select the search result
        let action = ctrl.on_key(MenuKey::Enter);
        assert!(matches!(action, MenuAction::Execute { .. }));
        if let MenuAction::Execute { params, .. } = action {
            assert_eq!(params["target_uri"], Value::String("target-block".into()));
        }
    }

    #[test]
    fn navigation_wraps_correctly() {
        let mut ctrl = CommandMenuController::new(test_ops(), context());
        ctrl.on_text_changed("/");

        let state = ctrl.menu_state().unwrap();
        let num_matches = state.matches.len();
        assert!(num_matches >= 2);

        // Move down
        ctrl.on_key(MenuKey::Down);
        assert_eq!(ctrl.menu_state().unwrap().selected_index, 1);

        // Move up
        ctrl.on_key(MenuKey::Up);
        assert_eq!(ctrl.menu_state().unwrap().selected_index, 0);

        // Can't go above 0
        ctrl.on_key(MenuKey::Up);
        assert_eq!(ctrl.menu_state().unwrap().selected_index, 0);
    }
}
