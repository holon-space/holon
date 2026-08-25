//! The effects a chevron click must perform, decided once for every frontend.
//!
//! Flipping an `expand_toggle` is not a single write: the view-local
//! expansion store is the only durable seed the gate is rebuilt from, and
//! `collapsed` is document state since the 2026-07-11 ruling. A frontend that
//! performs one leg and not the other either loses the gesture on the next
//! structural rebuild or leaves the document stale.

use holon_api::Value;
use holon_api::render_types::OperationWiring;
use holon_api::types::EntityName;

use crate::OperationIntent;
use crate::operations::find_set_field_op;

/// What a frontend must do after the user flips a toggle to `expanded`.
pub struct ExpandToggleEffects {
    /// `(target_id, expanded)` for `BuilderServices::set_block_expanded_view`
    /// — the store the builder re-seeds the gate from, and the only leg that
    /// survives a structural rebuild.
    pub view_store: (String, bool),
    /// `set_field("collapsed", !expanded)`, so collapse is undoable,
    /// provenance-tagged and syncs as document state. `None` when the node
    /// carries no `set_field` wiring, no entity name, or no row id (static
    /// design-gallery contexts) — the store leg still applies.
    pub intent: Option<OperationIntent>,
}

/// Decide the effects for flipping `target_id` to `expanded`.
pub fn expand_toggle_effects(
    target_id: &str,
    expanded: bool,
    operations: &[OperationWiring],
    entity_name: Option<&EntityName>,
    row_id: Option<&str>,
) -> ExpandToggleEffects {
    let intent = match (
        find_set_field_op("collapsed", operations),
        entity_name,
        row_id,
    ) {
        (Some(op), Some(entity), Some(id)) => Some(OperationIntent::set_field(
            entity,
            &op.name,
            id,
            "collapsed",
            Value::Boolean(!expanded),
        )),
        _ => None,
    };
    ExpandToggleEffects {
        view_store: (target_id.to_string(), expanded),
        intent,
    }
}

/// The persist leg of a `tree_item` chevron click, decided once for every
/// frontend like [`expand_toggle_effects`]: resolve the row's profile, take
/// its `set_field` operation's entity, and build the `set_field(collapsed)`
/// write the chevron dispatches. `None` (warned) when no profile resolves or
/// it carries no `set_field` — the fold is then view-local, like an unwired
/// `expand_toggle`.
pub fn tree_chevron_persist_intent(
    services: &dyn crate::reactive::BuilderServices,
    row: &holon_api::widget_spec::DataRow,
    row_id: &str,
    new_expanded: bool,
) -> Option<OperationIntent> {
    // Resolve off the FULL data row the tree_item interprets — a skinny
    // `{id}` probe starves the profile's computed-field conditions of their
    // declared columns (inv-no-declared-column-absent). `id` is pinned to
    // `row_id` so a synthetic row still resolves by scheme.
    let mut probe = row.clone();
    probe.insert("id".into(), Value::String(row_id.to_string()));
    let Some(profile) = services.resolve_profile(&probe) else {
        tracing::warn!(
            "tree_item chevron: resolve_profile None for row_id={row_id}; collapse will not \
             persist"
        );
        return None;
    };
    let Some(op) = profile.operations.iter().find(|o| o.name == "set_field") else {
        tracing::warn!(
            "tree_item chevron: set_field op not on profile for row_id={row_id}; collapse will \
             not persist"
        );
        return None;
    };
    Some(OperationIntent::set_field(
        &op.entity_name,
        "set_field",
        row_id,
        "collapsed",
        Value::Boolean(!new_expanded),
    ))
}

/// A `set_field("collapsed")` wiring as an `expand_toggle` node carries it.
/// Shared with `reactive_view_model`'s walk test so both sides of the seam —
/// deciding the effects and locating the node they are decided from — are
/// exercised against one fixture.
#[cfg(test)]
pub(crate) fn test_set_field_wiring() -> OperationWiring {
    use holon_api::render_types::OperationDescriptor;
    use holon_api::render_types::OperationParam;
    use holon_api::render_types::TypeHint;

    OperationWiring {
        modified_param: String::new(),
        descriptor: OperationDescriptor {
            entity_name: EntityName::new("block"),
            entity_short_name: "block".into(),
            name: "set_field".into(),
            display_name: "set_field".into(),
            required_params: vec![OperationParam {
                name: "value".into(),
                type_hint: TypeHint::Bool,
                description: String::new(),
            }],
            affected_fields: vec!["collapsed".to_string()],
            id_column: "id".to_string(),
            description: String::new(),
            param_mappings: vec![],
            target_scope: holon_api::TargetScope::Block,
            boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Test,
            },
            trigger: None,
            bound_params: Default::default(),
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_field_wiring() -> OperationWiring {
        test_set_field_wiring()
    }

    /// Expanding writes the store open AND clears `collapsed` on the row —
    /// the two legs the GPUI chevron and the headless driver both perform.
    #[test]
    fn expanding_writes_store_and_clears_collapsed() {
        let ops = vec![set_field_wiring()];
        let fx = expand_toggle_effects(
            "block:a",
            true,
            &ops,
            Some(&EntityName::new("block")),
            Some("block:a"),
        );
        assert_eq!(fx.view_store, ("block:a".to_string(), true));
        let intent = fx.intent.expect("set_field wiring present");
        assert_eq!(intent.op_name, "set_field");
        assert_eq!(
            intent.params.get("field"),
            Some(&Value::String("collapsed".into()))
        );
        assert_eq!(intent.params.get("value"), Some(&Value::Boolean(false)));
        assert_eq!(
            intent.params.get("id"),
            Some(&Value::String("block:a".into()))
        );
    }

    /// Collapsing is the inverse write: `collapsed = true`.
    #[test]
    fn collapsing_sets_collapsed_true() {
        let ops = vec![set_field_wiring()];
        let fx = expand_toggle_effects(
            "block:a",
            false,
            &ops,
            Some(&EntityName::new("block")),
            Some("block:a"),
        );
        assert_eq!(fx.view_store, ("block:a".to_string(), false));
        let intent = fx.intent.expect("set_field wiring present");
        assert_eq!(intent.params.get("value"), Some(&Value::Boolean(true)));
    }

    /// No wiring / no row id: the document write is skipped, but the toggle
    /// still folds locally because the store leg is unconditional.
    #[test]
    fn unwired_node_still_writes_the_store() {
        let fx = expand_toggle_effects("block:a", true, &[], None, None);
        assert_eq!(fx.view_store, ("block:a".to_string(), true));
        assert!(fx.intent.is_none());

        let ops = vec![set_field_wiring()];
        let no_row =
            expand_toggle_effects("block:a", true, &ops, Some(&EntityName::new("block")), None);
        assert!(no_row.intent.is_none());
    }
}
