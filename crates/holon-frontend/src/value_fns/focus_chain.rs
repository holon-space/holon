//! `focus_chain()` — reactive row set tracking the focused block.
//!
//! Takes no args. Emits one row per element in the focus chain. For
//! now the chain has at most one element (the currently focused
//! block); the parent walk lands when a synchronous parent-id lookup
//! lands (today's `get_block_data` only returns children).
//!
//! Columns: `id`, `uri`, `level`. `level` is `0` for the focused
//! block, `1` for its parent, etc. Empty when nothing is focused.
//!
//! Subscribes to `UiState.focused_block_mutable()` so the row set
//! re-emits whenever focus moves. The provider holds a `Mutable`
//! handle (cheap clone, shares state) so re-renders see the same Arc
//! via `ProviderCache` and refocusing reaches every subscriber via
//! the underlying signal graph.
//!
//! Caller pattern (mobile action bar):
//!
//! ```rhai
//! columns(#{collection: focus_chain(),
//!           item_template: columns(#{collection: ops_of(col("uri")),
//!                                    item_template: button(col("name"))})})
//! ```

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use futures_signals::signal::{Mutable, SignalExt};
use futures_signals::signal_vec::{SignalVec, SignalVecExt};

use holon_api::render_eval::ResolvedArgs;
use holon_api::widget_spec::DataRow;
use holon_api::{ptr_identity, EntityUri, InterpValue, ReactiveRowProvider, Value};

use crate::reactive::BuilderServices;
use crate::render_context::RenderContext;
use crate::render_interpreter::{RenderInterpreter, ValueFn};
use crate::value_fns::synthetic::SyntheticRows;
use crate::ReactiveViewModel;

/// `ReactiveRowProvider` backed by the focused-block `Mutable`.
/// Pure projection — no spawned task, no internal accumulator.
pub struct FocusChainProvider {
    focused: Mutable<Option<EntityUri>>,
}

impl FocusChainProvider {
    pub fn new(focused: Mutable<Option<EntityUri>>) -> Self {
        Self { focused }
    }
}

fn build_chain(focused: &Option<EntityUri>) -> Vec<Arc<DataRow>> {
    match focused {
        None => Vec::new(),
        Some(uri) => vec![Arc::new(focus_row(uri, 0))],
    }
}

fn focus_row(uri: &EntityUri, level: i64) -> DataRow {
    let s = uri.as_str().to_string();
    let mut row: HashMap<String, Value> = HashMap::new();
    row.insert("id".to_string(), Value::String(s.clone()));
    row.insert("uri".to_string(), Value::String(s));
    row.insert("level".to_string(), Value::Integer(level));
    row
}

impl ReactiveRowProvider for FocusChainProvider {
    fn rows_snapshot(&self) -> Vec<Arc<DataRow>> {
        build_chain(&self.focused.get_cloned())
    }

    fn rows_signal_vec(&self) -> Pin<Box<dyn SignalVec<Item = Arc<DataRow>> + Send>> {
        Box::pin(
            self.focused
                .signal_cloned()
                .map(|opt| build_chain(&opt))
                .to_signal_vec(),
        )
    }

    fn keyed_rows_signal_vec(
        &self,
    ) -> Pin<Box<dyn SignalVec<Item = (String, Arc<DataRow>)> + Send>> {
        Box::pin(
            self.focused
                .signal_cloned()
                .map(|opt| build_chain(&opt))
                .to_signal_vec()
                .map(|row| {
                    let id = row
                        .get("id")
                        .and_then(|v| v.as_string())
                        .unwrap_or_default()
                        .to_string();
                    (id, row)
                }),
        )
    }

    fn cache_identity(&self) -> u64 {
        ptr_identity(self)
    }
}

struct FocusChainValueFn;

impl ValueFn for FocusChainValueFn {
    fn invoke(
        &self,
        args: &ResolvedArgs,
        services: &dyn BuilderServices,
        _ctx: &RenderContext,
    ) -> InterpValue {
        let focused = match services.focused_block_mutable() {
            Some(f) => f,
            None => {
                return InterpValue::Rows(Arc::new(SyntheticRows::from_rows(Vec::new())));
            }
        };
        let provider: Arc<dyn ReactiveRowProvider> = match services.provider_cache() {
            Some(cache) => cache.get_or_create("focus_chain", args, || {
                Arc::new(FocusChainProvider::new(focused.clone()))
            }),
            None => Arc::new(FocusChainProvider::new(focused)),
        };
        InterpValue::Rows(provider)
    }
}

/// Register `focus_chain` on the given interpreter. Collision-checked
/// by `register_value_fn`.
pub fn register_focus_chain(interp: &mut RenderInterpreter<ReactiveViewModel>) {
    interp.register_value_fn("focus_chain", FocusChainValueFn);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_is_empty_when_nothing_focused() {
        let provider = FocusChainProvider::new(Mutable::new(None));
        assert!(provider.rows_snapshot().is_empty());
    }

    #[test]
    fn snapshot_emits_one_row_at_level_zero() {
        let uri = EntityUri::from_raw("block:abc");
        let provider = FocusChainProvider::new(Mutable::new(Some(uri.clone())));
        let rows = provider.rows_snapshot();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].get("uri").and_then(|v| v.as_string()),
            Some(uri.as_str())
        );
        assert_eq!(rows[0].get("level"), Some(&Value::Integer(0)));
    }

    #[test]
    fn snapshot_reacts_to_focus_change() {
        let focus = Mutable::new(None);
        let provider = FocusChainProvider::new(focus.clone());
        assert!(provider.rows_snapshot().is_empty());

        focus.set(Some(EntityUri::from_raw("block:next")));
        let rows = provider.rows_snapshot();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].get("id").and_then(|v| v.as_string()),
            Some("block:next")
        );
    }
}
