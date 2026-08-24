//! Editor-independent param collection for pointer-driven operations.
//!
//! An `op_button` click carries only the row `id`, but an operation may need
//! more (`integration.set_field` also needs `field` and `value`). This walks
//! the still-missing params one at a time, offering the choices each param kind
//! admits, and produces the finished [`OperationIntent`] once every param is
//! resolved. The overlay that renders the choices anchors at the button
//! (`frontends/gpui/src/render/builders/op_button.rs`); the slash-command menu
//! keeps its own async entity search in [`crate::command_provider`].
//!
//! Only param kinds with a FIXED, in-hand choice set are collectable here —
//! `Bool` and `OneOf`. Kinds that need a backend search (`EntityId`) or free
//! text entry (`String`, `Number`) have no pointer affordance yet and surface a
//! visible [`CollectStep::Unsupported`] rather than a silent no-op.

use std::collections::HashMap;
use std::collections::VecDeque;

use holon_api::Value;
use holon_api::render_types::OperationDescriptor;
use holon_api::render_types::OperationParam;
use holon_api::render_types::TypeHint;

use crate::operations::OperationIntent;

/// One pickable value for the current param.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamChoice {
    /// What the user reads on the choice.
    pub label: String,
    /// The value inserted for the param when this choice is picked.
    pub value: Value,
    /// A stable, id-safe token identifying this choice within its param — the
    /// overlay composes its element id from `(param_name, slug)`, and the
    /// windowed test clicks by that id.
    pub slug: String,
}

/// What the collector wants next.
#[derive(Debug, Clone)]
pub enum CollectStep {
    /// Offer `choices` for `param_name`; pick one with
    /// [`ParamCollector::pick`].
    Collect {
        param_name: String,
        choices: Vec<ParamChoice>,
    },
    /// Every param is resolved — dispatch this and close the overlay.
    Ready(OperationIntent),
    /// The next missing param cannot be collected by pointer here. Fail loud:
    /// the overlay shows `reason`; nothing is dispatched.
    Unsupported { param_name: String, reason: String },
}

/// Walks an operation's still-missing params to a finished intent.
#[derive(Debug, Clone)]
pub struct ParamCollector {
    entity_name: holon_api::EntityName,
    op_name: String,
    /// Params already satisfied (from context) plus those picked so far.
    resolved: HashMap<String, Value>,
    /// Still-missing params, in declaration order.
    remaining: VecDeque<OperationParam>,
}

impl ParamCollector {
    /// Seed from an operation descriptor and the context params a click carries
    /// (typically `{ id }`). Params the context already satisfies are resolved;
    /// the rest queue for collection in declaration order.
    pub fn for_op(op: &OperationDescriptor, ctx_params: &HashMap<String, Value>) -> Self {
        let matched = crate::operation_matcher::try_match_from_context(op, ctx_params);
        Self {
            entity_name: matched.descriptor.entity_name.clone(),
            op_name: matched.descriptor.name.clone(),
            resolved: matched.resolved_params,
            remaining: matched.missing_params.into_iter().collect(),
        }
    }

    /// Whether any param still needs collecting. `false` means the click can go
    /// straight to dispatch with no overlay.
    pub fn needs_collection(&self) -> bool {
        !self.remaining.is_empty()
    }

    /// What to do next: collect the front param's choices, report it
    /// uncollectable, or (nothing left) hand back the finished intent.
    pub fn current(&self) -> CollectStep {
        let Some(param) = self.remaining.front() else {
            return CollectStep::Ready(OperationIntent::new(
                self.entity_name.clone(),
                self.op_name.clone(),
                self.resolved.clone(),
            ));
        };
        match choices_for(param) {
            Some(choices) => CollectStep::Collect {
                param_name: param.name.clone(),
                choices,
            },
            None => CollectStep::Unsupported {
                param_name: param.name.clone(),
                reason: format!(
                    "{}.{} needs '{}' ({}), which cannot be picked from a button here",
                    self.entity_name.as_str(),
                    self.op_name,
                    param.name,
                    type_hint_label(&param.type_hint),
                ),
            },
        }
    }

    /// Resolve `param_name` to `value` and advance. The pick must be for the
    /// param the collector is currently offering — a stale pick (the overlay
    /// clicked a choice for a param already advanced past) is a bug, so this
    /// asserts rather than silently misfiling the value.
    pub fn pick(&mut self, param_name: &str, value: Value) {
        let front = self
            .remaining
            .front()
            .unwrap_or_else(|| panic!("pick('{param_name}') with no param awaiting collection"))
            .name
            .clone();
        assert_eq!(
            front, param_name,
            "pick('{param_name}') but the collector is awaiting '{front}'"
        );
        self.resolved.insert(param_name.to_string(), value);
        self.remaining.pop_front();
    }
}

/// The pointer choices a param kind admits, or `None` when it has no pointer
/// affordance (fail loud upstream).
fn choices_for(param: &OperationParam) -> Option<Vec<ParamChoice>> {
    match &param.type_hint {
        TypeHint::Bool => Some(vec![
            ParamChoice {
                label: "On".to_string(),
                value: Value::Boolean(true),
                slug: "true".to_string(),
            },
            ParamChoice {
                label: "Off".to_string(),
                value: Value::Boolean(false),
                slug: "false".to_string(),
            },
        ]),
        // One choice per admitted value — including the single-value case, which
        // is shown (not auto-picked) so every popup has a deterministic step.
        TypeHint::OneOf { values } => Some(
            values
                .iter()
                .map(|v| {
                    let label = choice_label(v);
                    ParamChoice {
                        slug: label.clone(),
                        label,
                        value: v.clone(),
                    }
                })
                .collect(),
        ),
        _ => None,
    }
}

/// A value's label on a choice. `OneOf` values are strings in practice; a
/// non-string value falls back to a debug form rather than vanishing.
fn choice_label(value: &Value) -> String {
    value
        .as_string()
        .map(str::to_string)
        .unwrap_or_else(|| format!("{value:?}"))
}

fn type_hint_label(hint: &TypeHint) -> &'static str {
    match hint {
        TypeHint::Bool => "bool",
        TypeHint::String => "text",
        TypeHint::Number => "number",
        TypeHint::EntityId { .. } => "entity reference",
        TypeHint::OneOf { .. } => "one-of",
        TypeHint::Object { .. } => "object",
        TypeHint::Expr => "expression",
        TypeHint::Collection => "collection",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn param(name: &str, hint: TypeHint) -> OperationParam {
        OperationParam {
            name: name.to_string(),
            type_hint: hint,
            description: String::new(),
        }
    }

    fn descriptor(entity: &str, name: &str, params: Vec<OperationParam>) -> OperationDescriptor {
        OperationDescriptor {
            entity_name: holon_api::EntityName::new(entity),
            entity_short_name: entity.into(),
            name: name.into(),
            display_name: name.into(),
            required_params: params,
            param_mappings: vec![],
            id_column: "id".to_string(),
            description: String::new(),
            affected_fields: vec![],
            target_scope: holon_api::TargetScope::Global,
            boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Test,
            },
            trigger: None,
            bound_params: Default::default(),
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        }
    }

    /// `set_field`'s exact shape: id (String, resolved from ctx), field
    /// (OneOf["enabled"]), value (Bool).
    fn set_field_descriptor() -> OperationDescriptor {
        descriptor(
            "integration",
            "set_field",
            vec![
                param("id", TypeHint::String),
                param(
                    "field",
                    TypeHint::OneOf {
                        values: vec![Value::String("enabled".to_string())],
                    },
                ),
                param("value", TypeHint::Bool),
            ],
        )
    }

    fn ctx_id(id: &str) -> HashMap<String, Value> {
        HashMap::from([("id".to_string(), Value::String(id.to_string()))])
    }

    #[test]
    fn resolves_context_params_and_queues_the_rest() {
        let c = ParamCollector::for_op(&set_field_descriptor(), &ctx_id("integration:gcal"));
        assert!(c.needs_collection());
        match c.current() {
            CollectStep::Collect {
                param_name,
                choices,
            } => {
                // field is first-missing, and its single OneOf value is shown.
                assert_eq!(param_name, "field");
                assert_eq!(choices.len(), 1);
                assert_eq!(choices[0].slug, "enabled");
                assert_eq!(choices[0].value, Value::String("enabled".to_string()));
            }
            other => panic!("expected field collection, got {other:?}"),
        }
    }

    #[test]
    fn bool_param_offers_on_and_off() {
        let mut c = ParamCollector::for_op(&set_field_descriptor(), &ctx_id("integration:gcal"));
        c.pick("field", Value::String("enabled".to_string()));
        match c.current() {
            CollectStep::Collect {
                param_name,
                choices,
            } => {
                assert_eq!(param_name, "value");
                let slugs: Vec<&str> = choices.iter().map(|c| c.slug.as_str()).collect();
                assert_eq!(slugs, vec!["true", "false"]);
                assert_eq!(choices[0].value, Value::Boolean(true));
                assert_eq!(choices[1].value, Value::Boolean(false));
            }
            other => panic!("expected value collection, got {other:?}"),
        }
    }

    #[test]
    fn two_step_sequence_ends_in_a_merged_intent() {
        let mut c = ParamCollector::for_op(&set_field_descriptor(), &ctx_id("integration:gcal"));
        c.pick("field", Value::String("enabled".to_string()));
        c.pick("value", Value::Boolean(false));
        assert!(!c.needs_collection());
        match c.current() {
            CollectStep::Ready(intent) => {
                assert_eq!(intent.entity_name.as_str(), "integration");
                assert_eq!(intent.op_name, "set_field");
                assert_eq!(
                    intent.params.get("id"),
                    Some(&Value::String("integration:gcal".to_string()))
                );
                assert_eq!(
                    intent.params.get("field"),
                    Some(&Value::String("enabled".to_string()))
                );
                assert_eq!(intent.params.get("value"), Some(&Value::Boolean(false)));
            }
            other => panic!("expected ready intent, got {other:?}"),
        }
    }

    #[test]
    fn a_fully_satisfied_op_needs_no_collection() {
        let mut ctx = ctx_id("integration:gcal");
        ctx.insert("field".to_string(), Value::String("enabled".to_string()));
        ctx.insert("value".to_string(), Value::Boolean(true));
        let c = ParamCollector::for_op(&set_field_descriptor(), &ctx);
        assert!(!c.needs_collection());
        assert!(matches!(c.current(), CollectStep::Ready(_)));
    }

    #[test]
    fn an_uncollectable_kind_fails_loud_not_silent() {
        let op = descriptor(
            "doc",
            "link",
            vec![
                param("id", TypeHint::String),
                param(
                    "target",
                    TypeHint::EntityId {
                        entity_name: "doc".into(),
                    },
                ),
            ],
        );
        let c = ParamCollector::for_op(&op, &ctx_id("doc:1"));
        match c.current() {
            CollectStep::Unsupported { param_name, reason } => {
                assert_eq!(param_name, "target");
                assert!(reason.contains("entity reference"), "reason: {reason}");
            }
            other => panic!("expected unsupported, got {other:?}"),
        }
    }
}
