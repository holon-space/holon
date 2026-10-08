//! `ops_of(uri)` — enumerate operations registered for a URI's scheme.
//!
//! Takes one positional arg — a URI string like `"block:…"`. Returns a
//! reactive row set with one row per registered operation. Columns:
//! `id`, `name`, `display_name`, `description`, `entity_name`,
//! `target_id` (= input URI), `icon`.
//!
//! Under the hood this asks `services.entity_operations(scheme)` — the
//! operation set every profile of that entity carries — and flattens it into
//! synthetic rows, minus what the write tier refuses on `uri`.
//!
//! Caller pattern:
//!
//! ```rhai
//! list(#{collection: ops_of(col("id")),
//!        item_template: button(col("name"))})
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use holon_api::EntityUri;
use holon_api::InterpValue;
use holon_api::Value;
use holon_api::computation::ComputeError;
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
    ) -> Result<InterpValue, ComputeError> {
        let arg = args.positional.first().cloned().unwrap_or(Value::Null);
        let not_a_uri = |why: String| ComputeError::WrongType {
            context: format!("ops_of's first argument ({why})"),
            expected: "an entity uri (`scheme:id`)",
            value: arg.clone(),
        };
        let subject = match arg.as_string().map(EntityUri::parse) {
            Some(Ok(subject)) => subject,
            Some(Err(e)) => return Err(not_a_uri(e.to_string())),
            None => return Err(not_a_uri("not a string".to_string())),
        };

        // `surface: "action_bar"` narrows to the ops that declared themselves
        // reachable from the mobile bar. Absent (the settings integration rows)
        // means every admitted op, which is that surface's whole point.
        let action_bar_only = args
            .named
            .get("surface")
            .and_then(|v| v.as_string().map(|s| s == ACTION_BAR_SURFACE))
            .unwrap_or(false);
        let Offer {
            mut ops,
            tier_withheld,
        } = resolve_ops(&subject, services);
        if action_bar_only {
            ops.retain(on_action_bar);
        }
        let row = ctx.row();
        let build = || -> Arc<dyn holon_api::ReactiveRowProvider> {
            Arc::new(SyntheticRows::from_rows(rows_from_ops(
                &ops,
                subject.as_str(),
                row,
            )))
        };

        // The key carries the write tier's verdict, so a re-homed subject is
        // never answered with the offer its old home allowed.
        let cache_name = if tier_withheld {
            "ops_of/write-tier-withheld"
        } else {
            "ops_of"
        };
        let provider: Arc<dyn holon_api::ReactiveRowProvider> = match services.provider_cache() {
            Some(cache) if caches_rows(&ops) => cache.get_or_create(cache_name, args, build),
            _ => build(),
        };
        Ok(InterpValue::Rows(provider))
    }
}

/// Build operation rows for a URI. Shared with `chain_ops` so the
/// composition shortcut produces identical row shapes.
pub fn ops_rows_for_uri(
    uri: &EntityUri,
    services: &dyn BuilderServices,
    row: &DataRow,
) -> Vec<Arc<DataRow>> {
    rows_from_ops(&resolve_ops(uri, services).ops, uri.as_str(), row)
}

/// [`ops_rows_for_uri`] narrowed to the operations that declared themselves
/// reachable from the mobile action bar (`SurfaceSet::action_bar`).
///
/// A block advertises dozens of operations; a phone bar has room for a handful
/// and every button costs the user a decision. Which ones belong there is the
/// op's own call, declared at its descriptor next to its slash-menu exposure,
/// so the two surfaces never drift into parallel lists.
pub fn action_bar_rows_for_uri(
    uri: &EntityUri,
    services: &dyn BuilderServices,
    row: &DataRow,
) -> Vec<Arc<DataRow>> {
    let ops: Vec<OperationWiring> = resolve_ops(uri, services)
        .ops
        .into_iter()
        .filter(on_action_bar)
        .collect();
    rows_from_ops(&ops, uri.as_str(), row)
}

/// The `surface:` value that narrows an enumeration to the mobile action bar.
pub const ACTION_BAR_SURFACE: &str = "action_bar";

fn on_action_bar(op: &OperationWiring) -> bool {
    matches!(
        op.descriptor.menu_exposure,
        holon_api::MenuExposure::Listed { surfaces } if surfaces.action_bar
    )
}

fn rows_from_ops(ops: &[OperationWiring], uri: &str, row: &DataRow) -> Vec<Arc<DataRow>> {
    // Presentation dedup, one row per operation NAME. The catalog is the
    // dispatcher's unioned provider set, advertised without dedup — the
    // structural block ops are knowingly double-advertised by
    // `SqlBlockOperations` and `LoroBlockOperations` — so an undeduped set
    // paints every one of them twice. First occurrence wins, which is the same
    // provider the dispatcher's first-wins routing will actually reach, and the
    // same rule the slash menu applies in `build_command_items`. Keyed on the
    // name, never the label: two different ops may share a display label.
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    ops.iter()
        .filter(|w| admits(w, row))
        .filter(|w| seen.insert(w.descriptor.name.as_str()))
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

/// The operations a subject offers.
struct Offer {
    ops: Vec<OperationWiring>,
    /// The write tier refused `subject`, and `ops` lacks the operations it
    /// judges.
    tier_withheld: bool,
}

fn resolve_ops(subject: &EntityUri, services: &dyn BuilderServices) -> Offer {
    // An entity's operations follow from its scheme alone; resolving a whole
    // profile would evaluate computed fields over a row this caller lacks.
    let ops: Vec<OperationWiring> = services
        .entity_operations(subject.scheme())
        .into_iter()
        .map(|d| d.to_default_wiring())
        .collect();
    let judged =
        |w: &OperationWiring| w.descriptor.entity_name.as_str() == holon_core::WRITE_TIER_ENTITY;
    // A clicked op dispatches with `subject` as its `id`, which is exactly what
    // the dispatcher's write tier judges.
    if !ops.iter().any(judged) || services.write_tier_admits(subject) {
        return Offer {
            ops,
            tier_withheld: false,
        };
    }
    Offer {
        ops: ops.into_iter().filter(|w| !judged(w)).collect(),
        tier_withheld: true,
    }
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
                optional_params: vec![],
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

    #[test]
    fn a_cached_offer_set_follows_a_write_tier_flip() {
        use holon_api::ReactiveRowProvider;

        let mut indent = descriptor("indent", OpGuard::None);
        indent.descriptor.entity_name = holon_core::WRITE_TIER_ENTITY.into();
        let profile = holon_api::RenderProfile {
            name: "block".to_string(),
            render: holon_api::render_dsl::parse_render_dsl(r#"text("x")"#).expect("parses"),
            operations: vec![indent.descriptor],
            variants: vec![],
        };
        let subject = holon_api::EntityUri::parse("block:recipe").expect("parses");
        let cache = Arc::new(crate::provider_cache::ProviderCache::new());
        let admitting = crate::reactive::StubBuilderServices::new()
            .with_profile(profile.clone())
            .with_provider_cache(cache.clone());
        let refusing = crate::reactive::StubBuilderServices::new()
            .with_profile(profile)
            .with_provider_cache(cache)
            .with_write_refused(subject.clone());
        let args = ResolvedArgs::from_positional_value(Value::String(subject.to_string()));
        let ctx = RenderContext::default();

        let rows = |v: InterpValue| match v {
            InterpValue::Rows(p) => p.rows_snapshot(),
            InterpValue::Value(v) => panic!("ops_of yields a row set, got {v:?}"),
        };
        let before = OpsOfValueFn
            .invoke(&args, &admitting, &ctx)
            .expect("a schemed uri resolves");
        let after = OpsOfValueFn
            .invoke(&args, &refusing, &ctx)
            .expect("a schemed uri resolves");
        assert_eq!(names(&rows(before)), vec!["indent".to_string()]);
        assert!(
            names(&rows(after)).is_empty(),
            "the tier now refuses {subject}, so the live cached offer must not be reused"
        );
    }

    #[test]
    fn a_scheme_less_argument_renders_an_error_naming_it() {
        let expr = holon_api::render_dsl::parse_render_dsl(
            r#"list(#{collection: ops_of("recipe"), item_template: text(col("name"))})"#,
        )
        .expect("parses");
        let tree = crate::reactive::interpret_pure(
            &expr,
            &[],
            &crate::reactive::StubBuilderServices::new(),
        );
        let message = tree.prop_str("message").unwrap_or_default();
        assert!(
            tree.is_error() && message.contains("\"recipe\""),
            "expected an error node naming the argument, got {:?}: {message}",
            tree.widget_name()
        );
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
