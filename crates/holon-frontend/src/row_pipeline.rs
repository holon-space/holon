//! Unified per-row interpretation pipeline.
//!
//! Three sites used to iterate `(rows × tmpl) → items`, each duplicating
//! parts of the per-row work:
//!
//! - `shared_tree_build` (eager, hierarchical) — applied `rules:` but
//!   skipped profile/ops resolution.
//! - The macro-generated static arm in `widget_builder.rs:270-291` (eager,
//!   flat) — resolved profile/ops but had no `rules:` support.
//! - The streaming `signal_vec` driver in `reactive_view.rs` (lazy, flat or
//!   hierarchical) — resolved profile/ops via `row_render_context` but had
//!   no `rules:` support.
//!
//! This module is the single per-row pipeline. It composes:
//!
//! 1. row context binding (`with_row`)
//! 2. profile resolution + ops wiring
//! 3. positional context (builder-specific: level/depth for tree,
//!    position/count/is_first/is_last for list, lane for board, ...)
//! 4. `rules:` evaluation against (row columns ∪ positional)
//! 5. flag merge (`overrides → ctx.flags`)
//! 6. interpret + attach_entity
//!
//! Builders pick their iteration shape (flat/hierarchical/grouped) and feed
//! each row through `apply_full_row_pipeline` (when they own context
//! construction) or `apply_rules_and_interpret_with_ctx` (when the caller
//! has already prepared a row context — e.g. the streaming driver wires
//! `parent_space` and the row mutability handle before calling).

use std::collections::HashMap;
use std::sync::Arc;

use holon_api::render_types::{OperationWiring, RenderExpr, RuleSpec};
use holon_api::widget_spec::DataRow;
use holon_api::Value;

use crate::reactive::BuilderServices;
use crate::render_interpreter::WithEntity;
use crate::RenderContext;

/// Parse a `rules:` argument value into a `Vec<RuleSpec>`.
///
/// The DSL passes the rules array as `Value::Array(Vec<Value::Object>)`;
/// each Object has `when` (a Predicate-shaped Object) and `override`
/// (a flat HashMap).
///
/// Returns an empty Vec on absent/malformed input — fail-soft because rules
/// are advisory metadata, not load-bearing semantics. Malformed rules
/// produce a `tracing::warn` so the misconfig surfaces.
pub fn parse_rules_arg(value: Option<&Value>) -> Vec<RuleSpec> {
    let Some(Value::Array(items)) = value else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Ok(json_val) = serde_json::to_value(item) else {
            tracing::warn!("rules: cannot serialize rule entry to JSON: {item:?}");
            continue;
        };
        match serde_json::from_value::<RuleSpec>(json_val) {
            Ok(spec) => out.push(spec),
            Err(e) => tracing::warn!("rules: malformed rule entry, ignoring: {item:?} ({e})"),
        }
    }
    out
}

/// Evaluate every rule's `when` predicate against the per-row context and
/// merge matching `override` maps in declaration order (later wins per key).
///
/// `positional` injects builder-specific keys (`level`/`depth` for tree,
/// `position`/`count`/`is_first`/`is_last` for list, etc.) alongside the
/// row's own columns. Predicates can reference either via `eq("level", 0)`
/// or `eq("status", "done")` interchangeably.
pub fn evaluate_rules_with_positional(
    rules: &[RuleSpec],
    positional: &HashMap<String, Value>,
    row: &Arc<DataRow>,
) -> HashMap<String, Value> {
    let mut ctx_map: HashMap<String, Value> = (**row).clone();
    for (k, v) in positional {
        ctx_map.insert(k.clone(), v.clone());
    }
    let mut merged = HashMap::new();
    for rule in rules {
        if rule.when.evaluate(&ctx_map) {
            for (k, v) in &rule.overrides {
                merged.insert(k.clone(), v.clone());
            }
        }
    }
    merged
}

/// Lower-level: apply rules + flags + interpret to an *already-built* row
/// context. Used by the streaming driver, where `parent_space` and the
/// row-mutability handle are wired into the context before this is called.
///
/// Returns `(node, overrides)` — the interpreted widget plus the
/// rule-override map (empty when no rules matched). Tree-style builders
/// thread the overrides into wrapper props (e.g. `tree_item.show_bullet`).
pub fn apply_rules_and_interpret_with_ctx<F, W>(
    row_ctx: RenderContext,
    template: &RenderExpr,
    rules: &[RuleSpec],
    row: &Arc<DataRow>,
    positional: HashMap<String, Value>,
    interpret: F,
) -> (W, HashMap<String, Value>)
where
    F: Fn(&RenderExpr, &RenderContext) -> W,
    W: WithEntity,
{
    let merged = if rules.is_empty() {
        HashMap::new()
    } else {
        evaluate_rules_with_positional(rules, &positional, row)
    };

    let row_ctx = if merged.is_empty() {
        row_ctx
    } else {
        let mut flags = row_ctx.flags.clone();
        flags.extend(merged.clone());
        row_ctx.with_flags(flags)
    };

    let mut node = interpret(template, &row_ctx);
    node.attach_entity(Arc::clone(row));

    (node, merged)
}

/// Higher-level: build the standard row context (with_row + profile + ops)
/// and apply rules + interpret. Used by builders that don't need
/// streaming-specific context wiring (parent_space, row mutability).
pub fn apply_full_row_pipeline<F, W>(
    services: &dyn BuilderServices,
    base_ctx: &RenderContext,
    template: &RenderExpr,
    rules: &[RuleSpec],
    row: &Arc<DataRow>,
    positional: HashMap<String, Value>,
    interpret: F,
) -> (W, HashMap<String, Value>)
where
    F: Fn(&RenderExpr, &RenderContext) -> W,
    W: WithEntity,
{
    let row_ctx = base_ctx.with_row(Arc::clone(row));
    let ops: Vec<OperationWiring> = services
        .resolve_profile(row_ctx.row())
        .map(|p| {
            p.operations
                .into_iter()
                .map(|d| d.to_default_wiring())
                .collect()
        })
        .unwrap_or_default();
    let row_ctx = if ops.is_empty() {
        row_ctx
    } else {
        row_ctx.with_operations(ops, services)
    };

    apply_rules_and_interpret_with_ctx(row_ctx, template, rules, row, positional, interpret)
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_api::predicate::Predicate;
    use holon_api::render_types::RuleSpec;

    fn row_with(pairs: &[(&str, Value)]) -> Arc<DataRow> {
        Arc::new(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        )
    }

    /// FU-6 regression: list-style positional context (`position`/`count`/
    /// `is_first`/`is_last`) is visible to predicates and drives per-row
    /// `override` merges. Mirrors what the macro static arm computes.
    #[test]
    fn list_positional_rules_fire_on_first_and_last() {
        let rules: Vec<RuleSpec> = vec![
            RuleSpec {
                when: Predicate::Eq {
                    field: "is_first".into(),
                    value: Value::Boolean(true),
                },
                overrides: HashMap::from([(
                    "role".to_string(),
                    Value::String("header".to_string()),
                )]),
            },
            RuleSpec {
                when: Predicate::Eq {
                    field: "is_last".into(),
                    value: Value::Boolean(true),
                },
                overrides: HashMap::from([(
                    "role".to_string(),
                    Value::String("footer".to_string()),
                )]),
            },
        ];

        let positional_for = |i: i64, count: i64| -> HashMap<String, Value> {
            HashMap::from([
                ("position".to_string(), Value::Integer(i)),
                ("count".to_string(), Value::Integer(count)),
                ("is_first".to_string(), Value::Boolean(i == 0)),
                ("is_last".to_string(), Value::Boolean(i + 1 == count)),
            ])
        };

        let row_first = row_with(&[("id", Value::String("a".into()))]);
        let row_middle = row_with(&[("id", Value::String("b".into()))]);
        let row_last = row_with(&[("id", Value::String("c".into()))]);

        let m_first = evaluate_rules_with_positional(&rules, &positional_for(0, 3), &row_first);
        assert_eq!(
            m_first.get("role"),
            Some(&Value::String("header".to_string()))
        );

        let m_middle = evaluate_rules_with_positional(&rules, &positional_for(1, 3), &row_middle);
        assert!(
            m_middle.is_empty(),
            "middle row should match neither is_first nor is_last; got {m_middle:?}"
        );

        let m_last = evaluate_rules_with_positional(&rules, &positional_for(2, 3), &row_last);
        assert_eq!(
            m_last.get("role"),
            Some(&Value::String("footer".to_string()))
        );
    }

    /// Tree-style positional context (`level`/`depth`) — same evaluator,
    /// different keys. This is the path `shared_tree_build` takes.
    #[test]
    fn tree_positional_rules_fire_on_level() {
        let rules: Vec<RuleSpec> = vec![RuleSpec {
            when: Predicate::Eq {
                field: "level".into(),
                value: Value::Integer(0),
            },
            overrides: HashMap::from([(
                "role".to_string(),
                Value::String("page_title".to_string()),
            )]),
        }];

        let row = row_with(&[("id", Value::String("doc".into()))]);

        let positional_root = HashMap::from([
            ("level".to_string(), Value::Integer(0)),
            ("depth".to_string(), Value::Integer(0)),
        ]);
        let merged_root = evaluate_rules_with_positional(&rules, &positional_root, &row);
        assert_eq!(
            merged_root.get("role"),
            Some(&Value::String("page_title".to_string()))
        );

        let positional_child = HashMap::from([
            ("level".to_string(), Value::Integer(1)),
            ("depth".to_string(), Value::Integer(1)),
        ]);
        let merged_child = evaluate_rules_with_positional(&rules, &positional_child, &row);
        assert!(merged_child.is_empty());
    }

    /// Rules can also reference the row's own columns (the union of row
    /// data + positional). Predicates like `eq("status", "done")` work
    /// the same as `eq("position", 0)`.
    #[test]
    fn rules_can_match_row_columns() {
        let rules: Vec<RuleSpec> = vec![RuleSpec {
            when: Predicate::Eq {
                field: "status".into(),
                value: Value::String("done".into()),
            },
            overrides: HashMap::from([(
                "role".to_string(),
                Value::String("completed".to_string()),
            )]),
        }];

        let row_done = row_with(&[
            ("id", Value::String("a".into())),
            ("status", Value::String("done".into())),
        ]);
        let row_open = row_with(&[
            ("id", Value::String("b".into())),
            ("status", Value::String("todo".into())),
        ]);

        let positional = HashMap::new();
        let m_done = evaluate_rules_with_positional(&rules, &positional, &row_done);
        assert_eq!(
            m_done.get("role"),
            Some(&Value::String("completed".to_string()))
        );

        let m_open = evaluate_rules_with_positional(&rules, &positional, &row_open);
        assert!(m_open.is_empty());
    }

    /// Last rule wins per key when multiple rules match — declaration order
    /// matters. Mirrors the original `shared_tree_build` semantics.
    #[test]
    fn later_rule_wins_per_override_key() {
        let rules: Vec<RuleSpec> = vec![
            RuleSpec {
                when: Predicate::Always,
                overrides: HashMap::from([(
                    "role".to_string(),
                    Value::String("first".to_string()),
                )]),
            },
            RuleSpec {
                when: Predicate::Always,
                overrides: HashMap::from([(
                    "role".to_string(),
                    Value::String("second".to_string()),
                )]),
            },
        ];

        let row = row_with(&[("id", Value::String("x".into()))]);
        let merged = evaluate_rules_with_positional(&rules, &HashMap::new(), &row);
        assert_eq!(
            merged.get("role"),
            Some(&Value::String("second".to_string()))
        );
    }
}
