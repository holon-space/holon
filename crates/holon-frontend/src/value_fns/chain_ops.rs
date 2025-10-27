//! `chain_ops(level)` — composition convenience: ops registered for
//! the URI at the given level of the focus chain.
//!
//! Equivalent in spirit to `ops_of(focus_chain()[level].uri)`. Takes
//! one positional integer arg `level` (0 = focused, 1 = parent, ...).
//! Returns an empty row set for levels not present in the chain.
//!
//! Composability example used by the mobile action bar:
//!
//! ```rhai
//! columns(#{collection: focus_chain(),
//!           item_template: columns(#{collection: chain_ops(col("level")),
//!                                    item_template: button(col("name"))})})
//! ```
//!
//! Today the focus chain has at most one element (the focused block),
//! so `chain_ops(0)` mirrors `ops_of(focused_uri)` and `chain_ops(N>0)`
//! is empty. The behaviour generalises automatically once
//! `focus_chain` learns to walk parents.

use std::sync::Arc;

use holon_api::render_eval::ResolvedArgs;
use holon_api::widget_spec::DataRow;
use holon_api::{EntityUri, InterpValue, Value};

use crate::reactive::BuilderServices;
use crate::render_context::RenderContext;
use crate::render_interpreter::{RenderInterpreter, ValueFn};
use crate::value_fns::ops_of::ops_rows_for_uri;
use crate::value_fns::synthetic::SyntheticRows;
use crate::ReactiveViewModel;

struct ChainOpsValueFn;

impl ValueFn for ChainOpsValueFn {
    fn invoke(
        &self,
        args: &ResolvedArgs,
        services: &dyn BuilderServices,
        _ctx: &RenderContext,
    ) -> InterpValue {
        let level = args.positional.first().and_then(value_to_i64).unwrap_or(0);

        let chain = current_focus_chain(services);
        let uri_opt: Option<EntityUri> = chain.get(level as usize).cloned();

        let provider: Arc<dyn holon_api::ReactiveRowProvider> = match services.provider_cache() {
            Some(cache) => {
                let uri_for_ctor = uri_opt.clone();
                cache.get_or_create("chain_ops", args, || {
                    let rows: Vec<Arc<DataRow>> = match uri_for_ctor {
                        Some(uri) => ops_rows_for_uri(uri.as_str(), services),
                        None => Vec::new(),
                    };
                    Arc::new(SyntheticRows::from_rows(rows))
                })
            }
            None => {
                let rows: Vec<Arc<DataRow>> = match uri_opt {
                    Some(uri) => ops_rows_for_uri(uri.as_str(), services),
                    None => Vec::new(),
                };
                Arc::new(SyntheticRows::from_rows(rows))
            }
        };

        InterpValue::Rows(provider)
    }
}

fn value_to_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Integer(i) => Some(*i),
        Value::Float(f) => Some(*f as i64),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// Snapshot of the focus chain — focused block first, then parents.
/// Today returns at most one entry (parent walk lands with the
/// matching `focus_chain` upgrade).
fn current_focus_chain(services: &dyn BuilderServices) -> Vec<EntityUri> {
    services
        .focused_block_mutable()
        .and_then(|m| m.get_cloned())
        .map(|uri| vec![uri])
        .unwrap_or_default()
}

/// Register `chain_ops` on the given interpreter. Collision-checked
/// by `register_value_fn`.
pub fn register_chain_ops(interp: &mut RenderInterpreter<ReactiveViewModel>) {
    interp.register_value_fn("chain_ops", ChainOpsValueFn);
}
