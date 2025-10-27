//! Shared helpers for OperationProvider dispatch logic
//!
//! This module provides utilities to reduce duplication between
//! `fake_wrapper.rs` and `provider_wrapper.rs`.

use holon::core::datasource::UndoAction;

/// Transform an UndoAction by setting the entity_name field
pub fn transform_undo_action(action: UndoAction, entity_name: &str) -> UndoAction {
    match action {
        UndoAction::Undo(mut op) => {
            op.entity_name = entity_name.into();
            UndoAction::Undo(op)
        }
        UndoAction::Irreversible => UndoAction::Irreversible,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_api::Operation;

    #[test]
    fn test_transform_undo_action_with_undo() {
        let op = Operation::new("", "delete", "Delete", std::collections::HashMap::new());
        let action = UndoAction::Undo(op);
        let result = transform_undo_action(action, "todoist_task");

        if let UndoAction::Undo(op) = result {
            assert_eq!(op.entity_name, "todoist_task");
        } else {
            panic!("Expected Undo variant");
        }
    }

    #[test]
    fn test_transform_undo_action_irreversible() {
        let action = UndoAction::Irreversible;
        let result = transform_undo_action(action, "todoist_task");
        assert!(matches!(result, UndoAction::Irreversible));
    }
}
