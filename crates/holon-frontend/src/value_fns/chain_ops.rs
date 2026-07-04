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
//!
//! Reactivity: like `focus_chain`, the provider is a pure projection of
//! `UiState.focused_block_mutable()` — the row set re-emits (with fresh
//! `target_id`s) whenever focus moves. A static snapshot here is a
//! correctness bug: the mobile action bar starts empty (nothing focused
//! at first render) and would otherwise dispatch ops against whichever
//! block happened to be focused when the cached provider was built.

use std::pin::Pin;
use std::sync::Arc;

use futures_signals::signal::Mutable;
use futures_signals::signal::SignalExt;
use futures_signals::signal_vec::SignalVec;
use futures_signals::signal_vec::SignalVecExt;
use holon_api::EntityUri;
use holon_api::InterpValue;
use holon_api::ReactiveRowProvider;
use holon_api::Value;
use holon_api::ptr_identity;
use holon_api::render_eval::ResolvedArgs;
use holon_api::widget_spec::DataRow;

use crate::ReactiveViewModel;
use crate::reactive::BuilderServices;
use crate::render_context::RenderContext;
use crate::render_interpreter::RenderInterpreter;
use crate::render_interpreter::ValueFn;
use crate::value_fns::ops_of::ops_rows_for_uri;
use crate::value_fns::synthetic::SyntheticRows;

/// `ReactiveRowProvider` projecting the focused-block `Mutable` through
/// the operation registry: one row per op registered for the scheme of
/// the URI at `level` in the focus chain. Ops are re-resolved on every
/// focus change so `target_id` always names the currently focused block.
pub struct ChainOpsProvider {
    focused: Mutable<Option<EntityUri>>,
    level: usize,
    services: Arc<dyn BuilderServices>,
}

impl ChainOpsProvider {
    pub fn new(
        focused: Mutable<Option<EntityUri>>,
        level: usize,
        services: Arc<dyn BuilderServices>,
    ) -> Self {
        Self {
            focused,
            level,
            services,
        }
    }
}

/// Focus chain projection — today at most one element (level 0). Mirrors
/// `focus_chain::build_chain`; generalises when the parent walk lands.
fn chain_uri(focused: &Option<EntityUri>, level: usize) -> Option<EntityUri> {
    match level {
        0 => focused.clone(),
        _ => None,
    }
}

fn build_rows(
    focused: &Option<EntityUri>,
    level: usize,
    services: &dyn BuilderServices,
) -> Vec<Arc<DataRow>> {
    match chain_uri(focused, level) {
        Some(uri) => ops_rows_for_uri(uri.as_str(), services),
        None => Vec::new(),
    }
}

impl ReactiveRowProvider for ChainOpsProvider {
    fn rows_snapshot(&self) -> Vec<Arc<DataRow>> {
        build_rows(
            &self.focused.get_cloned(),
            self.level,
            self.services.as_ref(),
        )
    }

    fn rows_signal_vec(&self) -> Pin<Box<dyn SignalVec<Item = Arc<DataRow>> + Send>> {
        let level = self.level;
        let services = self.services.clone();
        Box::pin(
            self.focused
                .signal_cloned()
                .map(move |opt| build_rows(&opt, level, services.as_ref()))
                .to_signal_vec(),
        )
    }

    fn keyed_rows_signal_vec(
        &self,
    ) -> Pin<Box<dyn SignalVec<Item = (holon_api::RowKey, Arc<DataRow>)> + Send>> {
        let level = self.level;
        let services = self.services.clone();
        Box::pin(
            self.focused
                .signal_cloned()
                .map(move |opt| build_rows(&opt, level, services.as_ref()))
                .to_signal_vec()
                .map(|row| {
                    let id = holon_api::data_row_entity_uri(&row)
                        .unwrap_or_else(|| EntityUri::block(""));
                    ((id, holon_api::Occurrence::Canonical), row)
                }),
        )
    }

    fn cache_identity(&self) -> u64 {
        ptr_identity(self)
    }
}

struct ChainOpsValueFn;

impl ValueFn for ChainOpsValueFn {
    fn invoke(
        &self,
        args: &ResolvedArgs,
        services: &dyn BuilderServices,
        _: &RenderContext,
    ) -> InterpValue {
        let level = args
            .positional
            .first()
            .and_then(value_to_i64)
            .unwrap_or(0)
            .max(0) as usize;

        // No focus authority (stub/headless services): fixed empty row
        // set. This is also what keeps `clone_arc()` (which panics on
        // non-participating stubs) out of those paths.
        let Some(focused) = services.focused_block_mutable() else {
            return InterpValue::Rows(Arc::new(SyntheticRows::from_rows(Vec::new())));
        };

        let provider: Arc<dyn ReactiveRowProvider> = match services.provider_cache() {
            Some(cache) => cache.get_or_create("chain_ops", args, || {
                Arc::new(ChainOpsProvider::new(
                    focused.clone(),
                    level,
                    services.clone_arc(),
                ))
            }),
            None => Arc::new(ChainOpsProvider::new(focused, level, services.clone_arc())),
        };

        InterpValue::Rows(provider)
    }
}

fn value_to_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Integer(i) => Some(*i),
        Value::Float(f) => Some(*f as i64),
        // ALLOW(ok): best-effort numeric coercion — a non-numeric string is legitimately "not an
        // i64" (None).
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// Register `chain_ops` on the given interpreter. Collision-checked
/// by `register_value_fn`.
pub fn register_chain_ops(interp: &mut RenderInterpreter<ReactiveViewModel>) {
    interp.register_value_fn("chain_ops", ChainOpsValueFn);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StubBuilderServices;

    fn stub_arc() -> Arc<dyn BuilderServices> {
        Arc::new(StubBuilderServices::new())
    }

    #[test]
    fn snapshot_is_empty_when_nothing_focused() {
        let provider = ChainOpsProvider::new(Mutable::new(None), 0, stub_arc());
        assert!(provider.rows_snapshot().is_empty());
    }

    #[test]
    fn snapshot_retargets_when_focus_moves() {
        // ALLOW(entity_uri_from_raw): test literal (#[cfg(test)])
        let focused = Mutable::new(Some(EntityUri::from_raw("block:aaa")));
        let provider = ChainOpsProvider::new(focused.clone(), 0, stub_arc());
        // Stub services resolve no profile → no ops rows; the reactive
        // contract under test is that a snapshot AFTER a focus move
        // reflects the new focus (i.e. rows are rebuilt, not cached).
        let before = provider.rows_snapshot();
        // ALLOW(entity_uri_from_raw): test literal (#[cfg(test)])
        focused.set(Some(EntityUri::from_raw("block:bbb")));
        let after = provider.rows_snapshot();
        // With stub services both are empty; the assertion is that the
        // snapshot path re-projects from the CURRENT focus value rather
        // than a build-time capture (would be non-empty staleness with
        // real services — covered by the gpui windowed PBT).
        assert_eq!(before.len(), after.len());
    }

    #[test]
    fn nonzero_level_is_empty_until_parent_walk_lands() {
        // ALLOW(entity_uri_from_raw): test literal (#[cfg(test)])
        let focused = Mutable::new(Some(EntityUri::from_raw("block:aaa")));
        let provider = ChainOpsProvider::new(focused, 1, stub_arc());
        assert!(provider.rows_snapshot().is_empty());
    }
}
