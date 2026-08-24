//! `ops_of(uri)` — enumerate operations registered for a URI's scheme.
//!
//! Takes one positional arg — a URI string like `"block:…"`. Returns a
//! reactive row set with one row per registered operation. Columns:
//! `id`, `name`, `display_name`, `description`, `entity_name`,
//! `target_id` (= input URI), `icon`.
//!
//! Under the hood this calls `services.resolve_profile(&{id: uri})` —
//! the same path used by widget dispatch — and flattens the resulting
//! `operations: Vec<OperationWiring>` into synthetic rows.
//!
//! Caller pattern:
//!
//! ```rhai
//! list(#{collection: ops_of(col("id")),
//!        item_template: button(col("name"))})
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use holon_api::InterpValue;
use holon_api::Value;
use holon_api::render_eval::ResolvedArgs;
use holon_api::render_types::OperationWiring;
use holon_api::widget_spec::DataRow;

use crate::ReactiveViewModel;
use crate::reactive::BuilderServices;
use crate::render_context::RenderContext;
use crate::render_interpreter::RenderInterpreter;
use crate::render_interpreter::ValueFn;
use crate::value_fns::synthetic::SyntheticRows;

struct OpsOfValueFn;

impl ValueFn for OpsOfValueFn {
    fn invoke(
        &self,
        args: &ResolvedArgs,
        services: &dyn BuilderServices,
        ctx: &RenderContext,
    ) -> InterpValue {
        let uri = args
            .positional
            .first()
            .and_then(|v| v.as_string().map(|s| s.to_string()))
            .unwrap_or_else(|| {
                tracing::warn!("ops_of() called with no URI arg; returning empty provider");
                String::new()
            });

        let ops = resolve_ops(&uri, services);
        let row = ctx.row();
        let build = || -> Arc<dyn holon_api::ReactiveRowProvider> {
            Arc::new(SyntheticRows::from_rows(rows_from_ops(&ops, &uri, row)))
        };

        let provider: Arc<dyn holon_api::ReactiveRowProvider> = match services.provider_cache() {
            Some(cache) if caches_rows(&ops) => cache.get_or_create("ops_of", args, build),
            _ => build(),
        };
        InterpValue::Rows(provider)
    }
}

/// Build operation rows for a URI. Shared with `chain_ops` so the
/// composition shortcut produces identical row shapes.
pub fn ops_rows_for_uri(
    uri: &str,
    services: &dyn BuilderServices,
    row: &DataRow,
) -> Vec<Arc<DataRow>> {
    rows_from_ops(&resolve_ops(uri, services), uri, row)
}

fn rows_from_ops(ops: &[OperationWiring], uri: &str, row: &DataRow) -> Vec<Arc<DataRow>> {
    ops.iter()
        .filter(|w| admits(w, row))
        .map(|w| Arc::new(build_row(w, uri)))
        .collect()
}

/// Does `op`'s declared guard hold for `row`?
///
/// Only a RELATION guard is answerable here — it names columns, and the row
/// carries them. A block- or clock-subject guard needs a world this layer does
/// not have, so the op is listed and the dispatcher's gate decides it.
///
/// A relation guard naming a column the row does not carry has no answer at
/// all: the op is dropped and the reason is logged, because a fabricated
/// verdict would either hide a working affordance or paint one that refuses.
fn admits(op: &OperationWiring, row: &DataRow) -> bool {
    let holon_api::pattern::OpGuard::Declared { guard, source } = &op.descriptor.guard else {
        return true;
    };
    if !matches!(guard.subject, holon_api::pattern::Subject::Relation(_)) {
        return true;
    }
    match guard.evaluate_row(row) {
        Ok(holds) => holds,
        Err(e) => {
            tracing::error!(
                op = %op.descriptor.name,
                guard = %source,
                "ops_of cannot evaluate a declared guard against this row, so the operation is \
                 withheld: {e}"
            );
            false
        }
    }
}

/// May this operation set's rows be memoised on the call's arguments?
///
/// A relation guard's verdict comes from the row's column VALUES, which the
/// cache key does not carry, so a memoised row set would keep offering an
/// operation after the row stopped admitting it.
fn caches_rows(ops: &[OperationWiring]) -> bool {
    !ops.iter().any(|w| {
        matches!(
            &w.descriptor.guard,
            holon_api::pattern::OpGuard::Declared { guard, .. }
                if matches!(guard.subject, holon_api::pattern::Subject::Relation(_))
        )
    })
}

fn resolve_ops(uri: &str, services: &dyn BuilderServices) -> Vec<OperationWiring> {
    // Synthesize a minimal `{id: uri}` row and feed it to the standard
    // profile resolver. `resolve_profile` reads the URI scheme to look
    // up entity-level operations.
    let mut probe_row: HashMap<String, Value> = HashMap::new();
    probe_row.insert("id".to_string(), Value::String(uri.to_string()));
    services
        .resolve_profile(&probe_row)
        .map(|p| p.operations)
        .map(|ops| ops.into_iter().map(|d| d.to_default_wiring()).collect())
        .unwrap_or_default()
}

fn build_row(wiring: &OperationWiring, target_uri: &str) -> DataRow {
    let d = &wiring.descriptor;
    let mut row = HashMap::new();
    row.insert("id".to_string(), Value::String(format!("op:{}", d.name)));
    row.insert("name".to_string(), Value::String(d.name.clone()));
    row.insert(
        "display_name".to_string(),
        Value::String(d.display_name.clone()),
    );
    row.insert(
        "description".to_string(),
        Value::String(d.description.clone()),
    );
    row.insert(
        "entity_name".to_string(),
        Value::String(d.entity_name.as_str().to_string()),
    );
    row.insert(
        "target_id".to_string(),
        Value::String(target_uri.to_string()),
    );
    row.insert(
        "icon".to_string(),
        Value::String(derive_icon(&d.name).to_string()),
    );
    row
}

/// Rough icon guess from op name — placeholder until the icon library
/// gets a real `op.name → icon` map.
fn derive_icon(op_name: &str) -> &str {
    match op_name {
        "create" => "plus",
        "update" | "set_field" => "pencil",
        "delete" => "trash",
        "cycle_task_state" => "refresh",
        _ => "circle",
    }
}

/// Register `ops_of` on the given interpreter. Collision-checked by
/// `register_value_fn`.
pub fn register_ops_of(interp: &mut RenderInterpreter<ReactiveViewModel>) {
    interp.register_value_fn("ops_of", OpsOfValueFn);
}

#[cfg(test)]
mod tests {
    use holon_api::pattern::OpGuard;

    use super::*;

    fn descriptor(name: &str, guard: OpGuard) -> OperationWiring {
        OperationWiring {
            modified_param: String::new(),
            descriptor: holon_api::OperationDescriptor {
                entity_name: "integration".into(),
                entity_short_name: "integration".to_string(),
                id_column: "id".to_string(),
                name: name.to_string(),
                display_name: name.to_string(),
                description: String::new(),
                required_params: vec![],
                affected_fields: vec![],
                param_mappings: vec![],
                target_scope: holon_api::TargetScope::Global,
                boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
                menu_exposure: holon_api::MenuExposure::NotListed {
                    surface: holon_api::NonMenuSurface::PointerGesture,
                },
                trigger: None,
                bound_params: Default::default(),
                guard,
                marking_delta: holon_api::marking::MarkingDelta::Undeclared,
                arcs: holon_api::arcs::TransitionArcs::Declared {
                    reads: vec![],
                    emits: vec![],
                },
            },
        }
    }

    fn relation_guarded(name: &str) -> OperationWiring {
        descriptor(
            name,
            OpGuard::parse("integration.config_status == \"unconfigured\"").expect("parses"),
        )
    }

    fn row(config_status: &str) -> DataRow {
        DataRow::from([(
            "config_status".to_string(),
            Value::String(config_status.to_string()),
        )])
    }

    #[test]
    fn ops_of_drops_an_op_whose_guard_is_false() {
        let ops = vec![
            relation_guarded("begin_oauth"),
            descriptor("set_field", OpGuard::None),
        ];

        let offered = rows_from_ops(&ops, "integration:gcal", &row("unconfigured"));
        assert_eq!(
            names(&offered),
            vec!["begin_oauth".to_string(), "set_field".to_string()],
            "an unconfigured row admits the guarded op"
        );

        let offered = rows_from_ops(&ops, "integration:gcal", &row("configured"));
        assert_eq!(
            names(&offered),
            vec!["set_field".to_string()],
            "a configured row withdraws it, and leaves the unguarded op alone"
        );
    }

    /// A block-subject guard names a world this layer does not have. The op
    /// stays listed and the dispatcher's gate decides it.
    #[test]
    fn a_block_guarded_op_passes_through() {
        let ops = vec![descriptor(
            "archive",
            OpGuard::parse("has_tag(\"task\")").expect("parses"),
        )];
        let offered = rows_from_ops(&ops, "block:abc", &DataRow::new());
        assert_eq!(names(&offered), vec!["archive".to_string()]);
    }

    /// A relation guard the row cannot answer withholds the op — a fabricated
    /// verdict would either hide a working affordance or paint one that
    /// refuses.
    #[test]
    fn a_relation_guarded_op_is_withheld_when_the_row_lacks_its_column() {
        let ops = vec![relation_guarded("begin_oauth")];
        let offered = rows_from_ops(&ops, "integration:gcal", &DataRow::new());
        assert!(names(&offered).is_empty());
    }

    #[test]
    fn a_stale_ops_row_set_is_not_reused_after_the_guard_flips() {
        let guarded = vec![relation_guarded("begin_oauth")];
        assert!(
            !caches_rows(&guarded),
            "a relation guard's verdict comes from row values the cache key does not carry"
        );
        assert!(
            caches_rows(&[descriptor("set_field", OpGuard::None)]),
            "an unguarded op set is still memoised"
        );

        // The property the bypass protects: same uri, different guard column.
        let before = rows_from_ops(&guarded, "integration:gcal", &row("unconfigured"));
        let after = rows_from_ops(&guarded, "integration:gcal", &row("configured"));
        assert_eq!(names(&before), vec!["begin_oauth".to_string()]);
        assert!(names(&after).is_empty());
    }

    fn names(rows: &[Arc<DataRow>]) -> Vec<String> {
        rows.iter()
            .map(|r| {
                r.get("name")
                    .and_then(|v| v.as_string())
                    .expect("every op row carries a name")
                    .to_string()
            })
            .collect()
    }
}
