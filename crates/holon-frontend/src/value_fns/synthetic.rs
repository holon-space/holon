//! Shared building block for value-fn providers: a row set that is
//! populated once and then acts like any other `ReactiveRowProvider`.
//!
//! Used by `ops_of` (which enumerates operations for a URI scheme);
//! re-used by `focus_chain` / `chain_ops` when they land.

use std::pin::Pin;
use std::sync::Arc;

use futures_signals::signal_vec::MutableVec;
use futures_signals::signal_vec::SignalVec;
use futures_signals::signal_vec::SignalVecExt;
use holon_api::ReactiveRowProvider;
use holon_api::ptr_identity;
use holon_api::widget_spec::DataRow;

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
    ) -> Pin<Box<dyn SignalVec<Item = (holon_api::RowKey, Arc<DataRow>)> + Send>> {
        Box::pin(self.rows.signal_vec_cloned().map(|row| {
            let id = holon_api::RowIdentity::of_row(&*row).to_store_key();
            ((id, holon_api::Occurrence::Canonical), row)
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use futures_signals::signal_vec::SignalVecExt;
    use holon_api::ReactiveRowProvider;
    use holon_api::Value;
    use tokio_stream::StreamExt;

    use super::*;

    async fn keys_of(provider: SyntheticRows) -> Vec<String> {
        let mut stream = provider.keyed_rows_signal_vec().to_stream();
        let diff = stream.next().await.expect("the provider emits one diff");
        match diff {
            futures_signals::signal_vec::VecDiff::Replace { values } => {
                values.iter().map(|((id, _), _)| id.to_string()).collect()
            }
            other => panic!("expected the initial Replace diff, got {other:?}"),
        }
    }

    fn row(pairs: &[(&str, &str)]) -> Arc<DataRow> {
        let mut m: HashMap<String, Value> = HashMap::new();
        for (k, v) in pairs {
            m.insert((*k).into(), Value::String((*v).into()));
        }
        Arc::new(m)
    }

    /// Two rows that carry no `id` are two different rows. A stand-in
    /// `EntityUri::block("")` gave both the SAME key — `block:` parses, so
    /// nothing failed loudly — and the keyed diff stream aliased them.
    /// `RowIdentity` keys each on its own content instead.
    #[tokio::test]
    async fn two_id_less_rows_keep_two_identities() {
        let keys = keys_of(SyntheticRows::from_rows(vec![
            row(&[("content", "first")]),
            row(&[("content", "second")]),
        ]))
        .await;
        assert_eq!(keys.len(), 2);
        assert_ne!(keys[0], keys[1], "id-less rows collapsed onto one key");
        assert!(
            keys.iter().all(|k| k.starts_with("value:")),
            "an id-less row is a value row, got {keys:?}"
        );
    }

    /// An EMPTY `id` is the same case: `block:` is a valid URI, so the old
    /// default made every empty-id row one row.
    #[tokio::test]
    async fn two_empty_id_rows_keep_two_identities() {
        let keys = keys_of(SyntheticRows::from_rows(vec![
            row(&[("id", ""), ("content", "first")]),
            row(&[("id", ""), ("content", "second")]),
        ]))
        .await;
        assert_eq!(keys.len(), 2);
        assert_ne!(keys[0], keys[1], "empty-id rows collapsed onto one key");
        assert!(
            !keys.iter().any(|k| k == "block:"),
            "an empty id must not mint the phantom `block:` identity, got {keys:?}"
        );
    }

    #[tokio::test]
    async fn a_keyed_row_with_an_id_keeps_that_id_as_its_key() {
        let keys = keys_of(SyntheticRows::from_rows(vec![row(&[("id", "block:r1")])])).await;
        assert_eq!(keys, vec!["block:r1".to_string()]);
    }
}
