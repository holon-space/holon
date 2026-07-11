//! `RefWatchesMut` / `RefWatch`.

use holon_pbt_core::capabilities::RefWatch;
use holon_pbt_core::capabilities::RefWatchesMut;
use holon_pbt_core::capabilities::WatchRow;

use super::super::reference_state::ReferenceState;

impl RefWatchesMut for ReferenceState {
    type WatchSpec = crate::pbt::query::WatchSpec;
    fn insert_watch(&mut self, query_id: &str, spec: Self::WatchSpec) {
        self.mcp.active_watches.insert(query_id.to_string(), spec);
    }
    fn remove_watch(&mut self, query_id: &str) {
        self.mcp.active_watches.remove(query_id);
    }
}

impl RefWatch for ReferenceState {
    fn active_watch_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.mcp.active_watches.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Evaluate the watch query against the (already SUT-ID-space-resolved)
    /// block state and stringify each `Value` into the `WatchRow` shape.
    /// NULL/non-string values become `None`, exactly as `Value::as_string()`
    /// returns `None`.
    fn expected_watch_rows(&self, query_id: &str) -> Vec<WatchRow> {
        let Some(watch_spec) = self.mcp.active_watches.get(query_id) else {
            return Vec::new();
        };
        self.query_results(watch_spec)
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|(k, v)| (k, v.as_string().map(str::to_string)))
                    .collect()
            })
            .collect()
    }

    fn watch_query_columns(&self, query_id: &str) -> Vec<String> {
        self.mcp
            .active_watches
            .get(query_id)
            .map(|ws| ws.query.columns.clone())
            .unwrap_or_default()
    }
}
