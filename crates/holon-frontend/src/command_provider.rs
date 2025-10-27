//! Slash command provider for the unified popup menu.
//!
//! Implements `PopupProvider` to show available operations filtered by typed text.
//! Handles the two-phase flow: command list → param collection.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use futures_signals::signal::{Signal, SignalExt};
use futures_signals::signal_vec::SignalVec;
use holon_api::render_types::{OperationParam, OperationWiring, TypeHint};
use holon_api::Value;

use crate::operation_matcher::{self, MatchedOperation};
use crate::popup_menu::{PopupItem, PopupProvider, PopupResult};

/// Internal state for param collection sub-phase.
#[derive(Debug, Clone)]
struct ParamCollectionState {
    operation: MatchedOperation,
    param: OperationParam,
    search_results: Vec<PopupItem>,
}

/// Slash command provider.
///
/// Shows available operations matching the filter. When an operation with missing
/// entity params is selected, transitions to param collection sub-phase.
pub struct CommandProvider {
    operations: Vec<OperationWiring>,
    context_params: HashMap<String, Value>,
    /// If Some, we're in param collection mode.
    param_state: Arc<Mutex<Option<ParamCollectionState>>>,
}

impl CommandProvider {
    pub fn new(operations: Vec<OperationWiring>, context_params: HashMap<String, Value>) -> Self {
        Self {
            operations,
            context_params,
            param_state: Arc::new(Mutex::new(None)),
        }
    }

    pub fn build_command_items(
        operations: &[OperationWiring],
        context_params: &HashMap<String, Value>,
        filter: &str,
    ) -> Vec<PopupItem> {
        let all_matches = operation_matcher::find_satisfiable(operations, context_params);

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

        filtered
            .iter()
            .map(|m| PopupItem {
                id: m.operation_name().to_string(),
                label: m.descriptor.display_name.clone(),
                icon: None,
            })
            .collect()
    }

    fn find_matched_operation(
        operations: &[OperationWiring],
        context_params: &HashMap<String, Value>,
        op_name: &str,
    ) -> Option<MatchedOperation> {
        operation_matcher::find_satisfiable(operations, context_params)
            .into_iter()
            .find(|m| m.operation_name() == op_name)
    }
}

impl PopupProvider for CommandProvider {
    fn source(&self) -> &str {
        "command_menu"
    }

    fn candidates(
        &self,
        filter: Pin<Box<dyn Signal<Item = String> + Send + Sync>>,
    ) -> Pin<Box<dyn SignalVec<Item = PopupItem> + Send>> {
        let operations = self.operations.clone();
        let context_params = self.context_params.clone();
        let param_state = self.param_state.clone();

        let signal = filter.map(move |f| {
            let state = param_state.lock().unwrap();
            if let Some(ps) = state.as_ref() {
                // In param collection: show search results filtered by current text
                let f_lower = f.to_lowercase();
                ps.search_results
                    .iter()
                    .filter(|item| f.is_empty() || item.label.to_lowercase().contains(&f_lower))
                    .cloned()
                    .collect()
            } else {
                Self::build_command_items(&operations, &context_params, &f)
            }
        });

        Box::pin(signal.to_signal_vec())
    }

    fn on_select(&self, item: &PopupItem, _filter: &str) -> PopupResult {
        let mut state = self.param_state.lock().unwrap();

        if let Some(ps) = state.take() {
            // We're in param collection — the selected item is an entity
            let selected_id = item.id.clone();
            let mut params = ps.operation.resolved_params.clone();
            params.insert(ps.param.name.clone(), Value::String(selected_id));

            return PopupResult::Execute {
                entity_name: ps.operation.entity_name().clone(),
                op_name: ps.operation.operation_name().to_string(),
                params,
            };
        }

        // Command list phase — find the matched operation
        let matched =
            match Self::find_matched_operation(&self.operations, &self.context_params, &item.id) {
                Some(m) => m,
                None => return PopupResult::NotActive,
            };

        if matched.is_fully_satisfied() {
            return PopupResult::Execute {
                entity_name: matched.entity_name().clone(),
                op_name: matched.operation_name().to_string(),
                params: matched.resolved_params,
            };
        }

        // Has missing entity params — transition to param collection
        let entity_params = matched.entity_params_needed();
        if let Some(&(_, _entity_name)) = entity_params.first() {
            let first_missing = matched.missing_params[0].clone();
            *state = Some(ParamCollectionState {
                operation: matched,
                param: first_missing,
                search_results: vec![],
            });
            // PopupMenu will re-render with empty items; the frontend
            // should detect this state and issue a search query.
            // For now, return Updated to keep the menu open.
            return PopupResult::Updated;
        }

        PopupResult::NotActive
    }
}

/// Feed entity search results to the command provider for param collection.
///
/// Call this when the frontend has executed a search query and received results.
/// Converts raw row data to PopupItems and stores them in the param state.
pub fn set_search_results(provider: &CommandProvider, results: Vec<HashMap<String, Value>>) {
    let mut state = provider.param_state.lock().unwrap();
    if let Some(ps) = state.as_mut() {
        ps.search_results = results
            .iter()
            .map(|row| {
                let id = row
                    .get("id")
                    .and_then(|v| v.as_string())
                    .unwrap_or("")
                    .to_string();
                let label = row
                    .get("content")
                    .and_then(|v| v.as_string())
                    .unwrap_or("(untitled)")
                    .to_string();
                PopupItem {
                    id,
                    label,
                    icon: None,
                }
            })
            .collect();
    }
}

/// Check if the provider is currently in param collection phase.
pub fn is_collecting_params(provider: &CommandProvider) -> bool {
    provider.param_state.lock().unwrap().is_some()
}

/// Get the entity name being searched for during param collection.
pub fn param_search_entity(provider: &CommandProvider) -> Option<String> {
    let state = provider.param_state.lock().unwrap();
    state.as_ref().and_then(|ps| match &ps.param.type_hint {
        TypeHint::EntityId { entity_name } => Some(entity_name.to_string()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_api::render_types::{OperationDescriptor, OperationParam, TypeHint};
    use holon_api::types::EntityName;

    fn make_op(name: &str, display: &str, params: Vec<OperationParam>) -> OperationWiring {
        OperationWiring {
            modified_param: String::new(),
            descriptor: OperationDescriptor {
                entity_name: EntityName::new("block"),
                entity_short_name: "block".into(),
                name: name.into(),
                display_name: display.into(),
                required_params: params,
                ..Default::default()
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
    fn builds_filtered_items() {
        let items = CommandProvider::build_command_items(&test_ops(), &context(), "");
        // set_field needs 3 params, only id available → not fully matchable but still shows
        // delete + embed_entity show
        assert!(items.len() >= 2);
    }

    #[test]
    fn filter_narrows_items() {
        let items = CommandProvider::build_command_items(&test_ops(), &context(), "emb");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "Embed");
    }

    #[test]
    fn select_fully_satisfied_executes() {
        let provider = CommandProvider::new(test_ops(), context());
        let item = PopupItem {
            id: "delete".into(),
            label: "Delete".into(),
            icon: None,
        };
        let result = provider.on_select(&item, "del");
        match result {
            PopupResult::Execute {
                op_name, params, ..
            } => {
                assert_eq!(op_name, "delete");
                assert_eq!(params["id"], Value::String("block-1".into()));
            }
            other => panic!("Expected Execute, got {:?}", other),
        }
    }

    #[test]
    fn select_with_missing_params_enters_collection() {
        let provider = CommandProvider::new(test_ops(), context());
        let item = PopupItem {
            id: "embed_entity".into(),
            label: "Embed".into(),
            icon: None,
        };
        let result = provider.on_select(&item, "emb");
        assert!(matches!(result, PopupResult::Updated));
        assert!(is_collecting_params(&provider));
        assert_eq!(param_search_entity(&provider), Some("block".to_string()));
    }

    #[test]
    fn param_collection_select_executes() {
        let provider = CommandProvider::new(test_ops(), context());

        // First: select embed (enters param collection)
        let item = PopupItem {
            id: "embed_entity".into(),
            label: "Embed".into(),
            icon: None,
        };
        provider.on_select(&item, "emb");

        // Feed search results
        set_search_results(
            &provider,
            vec![HashMap::from([(
                "id".into(),
                Value::String("target-block".into()),
            )])],
        );

        // Select the search result
        let entity_item = PopupItem {
            id: "target-block".into(),
            label: "(untitled)".into(),
            icon: None,
        };
        let result = provider.on_select(&entity_item, "");
        match result {
            PopupResult::Execute { params, .. } => {
                assert_eq!(params["target_uri"], Value::String("target-block".into()));
            }
            other => panic!("Expected Execute, got {:?}", other),
        }
        assert!(!is_collecting_params(&provider));
    }
}
