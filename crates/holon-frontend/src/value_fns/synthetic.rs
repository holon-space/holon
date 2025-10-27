//! Shared building block for value-fn providers: a row set that is
//! populated once and then acts like any other `ReactiveRowProvider`.
//!
//! Used by `ops_of` (which enumerates operations for a URI scheme);
//! re-used by `focus_chain` / `chain_ops` when they land.

use std::pin::Pin;
use std::sync::Arc;

use futures_signals::signal_vec::{MutableVec, SignalVec, SignalVecExt};

use holon_api::widget_spec::DataRow;
use holon_api::{ptr_identity, ReactiveRowProvider};

/// A `ReactiveRowProvider` backed by a `MutableVec`. Callers push rows
/// at construction time (or later via `push`) and the provider exposes
/// them through the trait.
///
/// The `MutableVec` is reactive — later mutations emit `VecDiff`
/// through the driver. Good enough for providers whose row set changes
/// in response to upstream signal changes (the stateless constraint
/// documented on `ReactiveRowProvider`).
pub struct SyntheticRows {
    rows: MutableVec<Arc<DataRow>>,
}

impl SyntheticRows {
    pub fn from_rows(rows: impl IntoIterator<Item = Arc<DataRow>>) -> Self {
        let mv = MutableVec::new_with_values(rows.into_iter().collect());
        Self { rows: mv }
    }
}

impl ReactiveRowProvider for SyntheticRows {
    fn rows_snapshot(&self) -> Vec<Arc<DataRow>> {
        self.rows.lock_ref().iter().cloned().collect()
    }

    fn rows_signal_vec(&self) -> Pin<Box<dyn SignalVec<Item = Arc<DataRow>> + Send>> {
        Box::pin(self.rows.signal_vec_cloned())
    }

    fn keyed_rows_signal_vec(
        &self,
    ) -> Pin<Box<dyn SignalVec<Item = (String, Arc<DataRow>)> + Send>> {
        Box::pin(self.rows.signal_vec_cloned().map(|row| {
            let id = row
                .get("id")
                .and_then(|v| v.as_string())
                .unwrap_or_default()
                .to_string();
            (id, row)
        }))
    }

    fn cache_identity(&self) -> u64 {
        // Identity is the provider's struct address. Two providers built
        // from the same args via `ProviderCache` share identity through
        // the Arc; two distinct constructions have distinct identities
        // even if they carry identical rows — which is fine for widget
        // caching (different Arcs ⇒ different cache keys).
        ptr_identity(self)
    }
}
