//! `LaneFilteredProvider` — a `ReactiveRowProvider` that filters an upstream
//! provider's rows by `row[lane_field] == lane_value`.
//!
//! Used by the shadow `board` builder to wire one streaming collection per
//! lane. Each lane's card list updates reactively when the upstream row set
//! changes (e.g. a peer drag-drop, a CDC push, or local set_field dispatch).
//!
//! The filter compares the row's `lane_field` column as a string. Empty /
//! missing values match the empty-string lane (the shadow builder maps that
//! to `lane_label_default` at the title level — only the value comparison
//! matters here).
//!
//! # Identity
//!
//! `cache_identity` mixes the upstream identity with `lane_field` and
//! `lane_value` so two providers over the same upstream that filter on
//! different lanes have distinct cache identities. This is what lets the
//! `ProviderCache` deduplicate per-lane providers across re-interpretations
//! of the same board.

use std::pin::Pin;
use std::sync::Arc;

use futures_signals::signal_vec::{SignalVec, SignalVecExt};
use holon_api::widget_spec::DataRow;
use holon_api::ReactiveRowProvider;

pub struct LaneFilteredProvider {
    upstream: Arc<dyn ReactiveRowProvider>,
    lane_field: String,
    lane_value: String,
}

impl LaneFilteredProvider {
    pub fn new(
        upstream: Arc<dyn ReactiveRowProvider>,
        lane_field: impl Into<String>,
        lane_value: impl Into<String>,
    ) -> Self {
        Self {
            upstream,
            lane_field: lane_field.into(),
            lane_value: lane_value.into(),
        }
    }

    fn matches(&self, row: &DataRow) -> bool {
        row.get(&self.lane_field)
            .and_then(|v| v.as_string())
            .unwrap_or("")
            == self.lane_value
    }
}

impl ReactiveRowProvider for LaneFilteredProvider {
    fn rows_snapshot(&self) -> Vec<Arc<DataRow>> {
        self.upstream
            .rows_snapshot()
            .into_iter()
            .filter(|row| self.matches(row))
            .collect()
    }

    fn rows_signal_vec(&self) -> Pin<Box<dyn SignalVec<Item = Arc<DataRow>> + Send>> {
        let lane_field = self.lane_field.clone();
        let lane_value = self.lane_value.clone();
        Box::pin(self.upstream.rows_signal_vec().filter(move |row| {
            row.get(&lane_field)
                .and_then(|v| v.as_string())
                .unwrap_or("")
                == lane_value
        }))
    }

    fn keyed_rows_signal_vec(
        &self,
    ) -> Pin<Box<dyn SignalVec<Item = (String, Arc<DataRow>)> + Send>> {
        let lane_field = self.lane_field.clone();
        let lane_value = self.lane_value.clone();
        Box::pin(self.upstream.keyed_rows_signal_vec().filter(move |entry| {
            entry
                .1
                .get(&lane_field)
                .and_then(|v| v.as_string())
                .unwrap_or("")
                == lane_value
        }))
    }

    fn cache_identity(&self) -> u64 {
        // Mix upstream identity with `(lane_field, lane_value)` so two
        // lane providers over the same upstream have distinct identities.
        // FNV-style mix; collisions only matter for cache dedup, not
        // correctness.
        let mut h: u64 = 0xcbf29ce484222325;
        let mix = |h: &mut u64, x: u64| {
            *h ^= x;
            *h = h.wrapping_mul(0x100000001b3);
        };
        mix(&mut h, self.upstream.cache_identity());
        for byte in self.lane_field.as_bytes() {
            mix(&mut h, *byte as u64);
        }
        mix(&mut h, 0); // separator
        for byte in self.lane_value.as_bytes() {
            mix(&mut h, *byte as u64);
        }
        h
    }

    fn row_mutable(
        &self,
        id: &str,
    ) -> Option<futures_signals::signal::ReadOnlyMutable<Arc<DataRow>>> {
        // Pass through — the row's mutable cell is owned by the upstream
        // provider regardless of which lane it currently sits in.
        self.upstream.row_mutable(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value_fns::synthetic::SyntheticRows;
    use holon_api::Value;
    use std::collections::HashMap;

    fn row(id: &str, lane: &str) -> Arc<DataRow> {
        let mut m: HashMap<String, Value> = HashMap::new();
        m.insert("id".into(), Value::String(id.into()));
        m.insert("task_state".into(), Value::String(lane.into()));
        Arc::new(m)
    }

    #[test]
    fn snapshot_filters_by_lane_field() {
        let rows = vec![
            row("a", "TODO"),
            row("b", "DOING"),
            row("c", "DONE"),
            row("d", "TODO"),
        ];
        let upstream: Arc<dyn ReactiveRowProvider> = Arc::new(SyntheticRows::from_rows(rows));
        let lane = LaneFilteredProvider::new(upstream, "task_state", "TODO");
        let ids: Vec<String> = lane
            .rows_snapshot()
            .into_iter()
            .filter_map(|r| {
                r.get("id")
                    .and_then(|v| v.as_string().map(|s| s.to_string()))
            })
            .collect();
        assert_eq!(ids, vec!["a", "d"]);
    }

    #[test]
    fn empty_lane_value_matches_missing_field() {
        let mut m: HashMap<String, Value> = HashMap::new();
        m.insert("id".into(), Value::String("ghost".into()));
        let untagged = Arc::new(m);
        let upstream: Arc<dyn ReactiveRowProvider> =
            Arc::new(SyntheticRows::from_rows(vec![untagged, row("a", "TODO")]));
        let lane = LaneFilteredProvider::new(upstream, "task_state", "");
        let ids: Vec<String> = lane
            .rows_snapshot()
            .into_iter()
            .filter_map(|r| {
                r.get("id")
                    .and_then(|v| v.as_string().map(|s| s.to_string()))
            })
            .collect();
        assert_eq!(ids, vec!["ghost"]);
    }

    #[test]
    fn cache_identity_distinguishes_lanes() {
        let upstream: Arc<dyn ReactiveRowProvider> =
            Arc::new(SyntheticRows::from_rows(vec![row("a", "TODO")]));
        let a = LaneFilteredProvider::new(upstream.clone(), "task_state", "TODO");
        let b = LaneFilteredProvider::new(upstream.clone(), "task_state", "DOING");
        let c = LaneFilteredProvider::new(upstream.clone(), "status", "TODO");
        assert_ne!(a.cache_identity(), b.cache_identity());
        assert_ne!(a.cache_identity(), c.cache_identity());
    }
}
