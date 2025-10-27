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

use holon_api::render_eval::ResolvedArgs;
use holon_api::render_types::OperationWiring;
use holon_api::widget_spec::DataRow;
use holon_api::{InterpValue, Value};

use crate::reactive::BuilderServices;
use crate::render_context::RenderContext;
use crate::render_interpreter::{RenderInterpreter, ValueFn};
use crate::value_fns::synthetic::SyntheticRows;
use crate::ReactiveViewModel;

struct OpsOfValueFn;

impl ValueFn for OpsOfValueFn {
    fn invoke(
        &self,
        args: &ResolvedArgs,
        services: &dyn BuilderServices,
        _ctx: &RenderContext,
    ) -> InterpValue {
        let uri = args
            .positional
            .first()
            .and_then(|v| v.as_string().map(|s| s.to_string()))
            .unwrap_or_else(|| {
                tracing::warn!("ops_of() called with no URI arg; returning empty provider");
                String::new()
            });

        let provider: Arc<dyn holon_api::ReactiveRowProvider> = match services.provider_cache() {
            Some(cache) => {
                let rows_owner_uri = uri.clone();
                cache.get_or_create("ops_of", args, || {
                    let rows = ops_rows_for_uri(&rows_owner_uri, services);
                    Arc::new(SyntheticRows::from_rows(rows))
                })
            }
            None => {
                let rows = ops_rows_for_uri(&uri, services);
                Arc::new(SyntheticRows::from_rows(rows))
            }
        };
        InterpValue::Rows(provider)
    }
}

/// Build operation rows for a URI. Shared with `chain_ops` so the
/// composition shortcut produces identical row shapes.
pub fn ops_rows_for_uri(uri: &str, services: &dyn BuilderServices) -> Vec<Arc<DataRow>> {
    let ops = resolve_ops(uri, services);
    ops.iter()
        .map(|w| build_row(w, uri))
        .map(Arc::new)
        .collect()
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
