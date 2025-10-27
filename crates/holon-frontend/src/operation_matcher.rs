use std::collections::HashMap;

use holon_api::render_types::{OperationDescriptor, OperationParam, OperationWiring, TypeHint};
use holon_api::Value;

/// Result of matching an operation against available parameters.
#[derive(Debug, Clone)]
pub struct MatchedOperation {
    pub descriptor: OperationDescriptor,
    pub resolved_params: HashMap<String, Value>,
    pub missing_params: Vec<OperationParam>,
}

impl MatchedOperation {
    pub fn is_fully_satisfied(&self) -> bool {
        self.missing_params.is_empty()
    }

    pub fn operation_name(&self) -> &str {
        &self.descriptor.name
    }

    pub fn entity_name(&self) -> &str {
        self.descriptor.entity_name.as_str()
    }

    /// Missing params that need entity search (EntityId type hint).
    /// Returns (param_name, entity_name) pairs.
    pub fn entity_params_needed(&self) -> Vec<(&str, &str)> {
        self.missing_params
            .iter()
            .filter_map(|p| match &p.type_hint {
                TypeHint::EntityId { entity_name } => Some((p.name.as_str(), entity_name.as_str())),
                _ => None,
            })
            .collect()
    }
}

/// Find operations satisfiable with the given available params.
///
/// Ported from Flutter's `OperationMatcher.findSatisfiable()`.
///
/// Intent filtering: if gesture-specific params are present (those declared in
/// `param_mappings.from`), only operations that USE those params are considered.
/// This prevents e.g. `delete` from matching during drag-drop just because it
/// only needs `id`.
pub fn find_satisfiable(
    operations: &[OperationWiring],
    available_params: &HashMap<String, Value>,
) -> Vec<MatchedOperation> {
    let descriptors: Vec<&OperationDescriptor> =
        operations.iter().map(|ow| &ow.descriptor).collect();
    let filtered = filter_by_intent_params(&descriptors, available_params);

    let mut results: Vec<MatchedOperation> = filtered
        .into_iter()
        .filter_map(|op| try_match(op, available_params))
        .collect();

    results.sort_by(|a, b| {
        // Fully satisfied first
        match (a.is_fully_satisfied(), b.is_fully_satisfied()) {
            (true, false) => return std::cmp::Ordering::Less,
            (false, true) => return std::cmp::Ordering::Greater,
            _ => {}
        }
        // More resolved params = better
        let resolved_cmp = b.resolved_params.len().cmp(&a.resolved_params.len());
        if resolved_cmp != std::cmp::Ordering::Equal {
            return resolved_cmp;
        }
        // Fewer missing params = better
        a.missing_params.len().cmp(&b.missing_params.len())
    });

    results
}

/// Find the single best matching operation (if any).
pub fn find_best_match(
    operations: &[OperationWiring],
    available_params: &HashMap<String, Value>,
) -> Option<MatchedOperation> {
    find_satisfiable(operations, available_params)
        .into_iter()
        .next()
}

/// Find all fully satisfiable operations.
pub fn find_fully_satisfiable(
    operations: &[OperationWiring],
    available_params: &HashMap<String, Value>,
) -> Vec<MatchedOperation> {
    find_satisfiable(operations, available_params)
        .into_iter()
        .filter(|m| m.is_fully_satisfied())
        .collect()
}

fn filter_by_intent_params<'a>(
    operations: &[&'a OperationDescriptor],
    available_params: &HashMap<String, Value>,
) -> Vec<&'a OperationDescriptor> {
    // Collect all "intent param sources" — params that any operation maps from
    let mut intent_param_sources = std::collections::HashSet::new();
    for op in operations {
        for mapping in &op.param_mappings {
            intent_param_sources.insert(mapping.from.as_str());
        }
    }

    // Which intent params are actually present?
    let present: Vec<&str> = intent_param_sources
        .iter()
        .copied()
        .filter(|p| available_params.contains_key(*p))
        .collect();

    // No intent params present → return all operations
    if present.is_empty() {
        return operations.to_vec();
    }

    // Filter to operations that use at least one present intent param
    operations
        .iter()
        .copied()
        .filter(|op| {
            op.param_mappings
                .iter()
                .any(|m| present.contains(&m.from.as_str()))
        })
        .collect()
}

fn try_match(
    op: &OperationDescriptor,
    available: &HashMap<String, Value>,
) -> Option<MatchedOperation> {
    let mut resolved = HashMap::new();
    let mut missing = Vec::new();

    for param in &op.required_params {
        if let Some(value) = resolve_param(&param.name, op, available) {
            resolved.insert(param.name.clone(), value);
        } else {
            missing.push(param.clone());
        }
    }

    // Only return if we resolved at least something useful
    if resolved.is_empty() && !op.required_params.is_empty() {
        return None;
    }

    Some(MatchedOperation {
        descriptor: op.clone(),
        resolved_params: resolved,
        missing_params: missing,
    })
}

fn resolve_param(
    param_name: &str,
    op: &OperationDescriptor,
    available: &HashMap<String, Value>,
) -> Option<Value> {
    // Direct match
    if let Some(value) = available.get(param_name) {
        return Some(value.clone());
    }

    // Try param mappings
    for mapping in &op.param_mappings {
        if !mapping.provides.contains(&param_name.to_string()) {
            continue;
        }

        if let Some(source_value) = available.get(&mapping.from) {
            // Extract from structured source (e.g., tree_position map)
            if let Value::Object(map) = source_value {
                if let Some(val) = map.get(param_name) {
                    return Some(val.clone());
                }
            }
            // Use source directly if it provides a single thing
            if mapping.provides.len() == 1 {
                return Some(source_value.clone());
            }
        }

        // Check defaults
        if let Some(default_value) = mapping.defaults.get(param_name) {
            return Some(default_value.clone());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_api::render_types::{ParamMapping, WidgetType};
    use holon_api::types::EntityName;

    fn make_op(
        name: &str,
        params: Vec<OperationParam>,
        mappings: Vec<ParamMapping>,
    ) -> OperationWiring {
        OperationWiring {
            widget_type: WidgetType::Button,
            modified_param: String::new(),
            descriptor: OperationDescriptor {
                entity_name: EntityName::new("block"),
                entity_short_name: "block".into(),
                id_column: "id".into(),
                name: name.into(),
                display_name: name.into(),
                description: String::new(),
                required_params: params,
                affected_fields: vec![],
                param_mappings: mappings,
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

    #[test]
    fn direct_param_resolution() {
        let ops = vec![make_op(
            "set_field",
            vec![
                param("id", TypeHint::String),
                param("field", TypeHint::String),
                param("value", TypeHint::String),
            ],
            vec![],
        )];

        let available: HashMap<String, Value> = [
            ("id".into(), Value::String("block-1".into())),
            ("field".into(), Value::String("content".into())),
            ("value".into(), Value::String("hello".into())),
        ]
        .into();

        let matches = find_satisfiable(&ops, &available);
        assert_eq!(matches.len(), 1);
        assert!(matches[0].is_fully_satisfied());
    }

    #[test]
    fn missing_params_identified() {
        let ops = vec![make_op(
            "embed_entity",
            vec![
                param("id", TypeHint::String),
                param(
                    "target_uri",
                    TypeHint::EntityId {
                        entity_name: EntityName::new("block"),
                    },
                ),
            ],
            vec![],
        )];

        let available: HashMap<String, Value> =
            [("id".into(), Value::String("block-1".into()))].into();

        let matches = find_satisfiable(&ops, &available);
        assert_eq!(matches.len(), 1);
        assert!(!matches[0].is_fully_satisfied());
        assert_eq!(matches[0].missing_params.len(), 1);
        assert_eq!(matches[0].missing_params[0].name, "target_uri");

        let entity_params = matches[0].entity_params_needed();
        assert_eq!(entity_params.len(), 1);
        assert_eq!(entity_params[0], ("target_uri", "block"));
    }

    #[test]
    fn intent_filtering_excludes_non_intent_ops() {
        let move_op = make_op(
            "move_block",
            vec![
                param("id", TypeHint::String),
                param("parent_id", TypeHint::String),
            ],
            vec![ParamMapping {
                from: "tree_position".into(),
                provides: vec!["parent_id".into()],
                defaults: HashMap::new(),
            }],
        );
        let delete_op = make_op("delete", vec![param("id", TypeHint::String)], vec![]);

        let ops = vec![move_op, delete_op];

        // When tree_position is provided, delete should be filtered out
        let available: HashMap<String, Value> = [
            ("id".into(), Value::String("block-1".into())),
            (
                "tree_position".into(),
                Value::Object(HashMap::from([(
                    "parent_id".into(),
                    Value::String("parent-1".into()),
                )])),
            ),
        ]
        .into();

        let matches = find_satisfiable(&ops, &available);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].operation_name(), "move_block");
    }

    #[test]
    fn no_intent_params_returns_all() {
        let ops = vec![
            make_op("set_field", vec![param("id", TypeHint::String)], vec![]),
            make_op("delete", vec![param("id", TypeHint::String)], vec![]),
        ];

        let available: HashMap<String, Value> =
            [("id".into(), Value::String("block-1".into()))].into();

        let matches = find_satisfiable(&ops, &available);
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn param_mapping_with_defaults() {
        let ops = vec![make_op(
            "create",
            vec![
                param("id", TypeHint::String),
                param("parent_id", TypeHint::String),
                param("predecessor", TypeHint::String),
            ],
            vec![ParamMapping {
                from: "tree_position".into(),
                provides: vec!["parent_id".into(), "predecessor".into()],
                defaults: HashMap::from([("predecessor".into(), Value::String("__last__".into()))]),
            }],
        )];

        let available: HashMap<String, Value> = [
            ("id".into(), Value::String("new-block".into())),
            (
                "tree_position".into(),
                Value::Object(HashMap::from([(
                    "parent_id".into(),
                    Value::String("parent-1".into()),
                )])),
            ),
        ]
        .into();

        let matches = find_satisfiable(&ops, &available);
        assert_eq!(matches.len(), 1);
        assert!(matches[0].is_fully_satisfied());
        assert_eq!(
            matches[0].resolved_params["predecessor"],
            Value::String("__last__".into())
        );
    }

    #[test]
    fn sorted_by_satisfaction() {
        let fully_satisfied = make_op("simple", vec![param("id", TypeHint::String)], vec![]);
        let partially_satisfied = make_op(
            "complex",
            vec![
                param("id", TypeHint::String),
                param("target", TypeHint::String),
            ],
            vec![],
        );

        let ops = vec![partially_satisfied, fully_satisfied];
        let available: HashMap<String, Value> = [("id".into(), Value::String("x".into()))].into();

        let matches = find_satisfiable(&ops, &available);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].operation_name(), "simple"); // fully satisfied first
        assert_eq!(matches[1].operation_name(), "complex");
    }
}
