//! [`HistoryStore`] implementations (VisionGapAnalysis C2b, ADR 0024 P8).
//!
//! - [`TursoHistoryStore`] — the full path: a plain `block_history` SQL table,
//!   maintained from the op/effect stream, queryable typed *and* joinable
//!   directly by matviews/PRQL (Martin's ruling allows the SQL surface). A
//!   disclosed ephemeral cache: rebuildable, never authoritative (Layer 3/4).
//! - [`DegradedHistoryStore`] — org-standalone vaults with no Turso query
//!   substrate. Reads fail loud with a disclosed reason; `record` is a
//!   disclosed no-op. Mirrors the CRDT-vs-LWW capability split.

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use holon_api::HistoryEvent;
use holon_api::HistoryFidelity;
use holon_api::HistoryQuery;
use holon_api::HistoryStore;
use holon_api::Value;
use tokio::sync::OnceCell;

use crate::storage::DbHandle;

const CREATE_TABLE_SQL: &str = "\
CREATE TABLE IF NOT EXISTS block_history (
    seq INTEGER PRIMARY KEY,
    block_id TEXT NOT NULL,
    op_name TEXT NOT NULL,
    origin TEXT NOT NULL,
    transition_id TEXT,
    session_id TEXT,
    tool_call_id TEXT,
    field TEXT,
    new_value TEXT,
    at_millis INTEGER NOT NULL
)";

const CREATE_INDEX_BLOCK: &str =
    "CREATE INDEX IF NOT EXISTS idx_block_history_block ON block_history(block_id)";
const CREATE_INDEX_SESSION: &str =
    "CREATE INDEX IF NOT EXISTS idx_block_history_session ON block_history(session_id)";
const CREATE_INDEX_AT: &str =
    "CREATE INDEX IF NOT EXISTS idx_block_history_at ON block_history(at_millis)";

/// A Turso-projected [`HistoryStore`]. The relation is a real SQL table so it
/// is directly joinable; this type is the thin typed accessor over it.
pub struct TursoHistoryStore {
    db: DbHandle,
    fidelity: HistoryFidelity,
    schema: OnceCell<()>,
}

impl TursoHistoryStore {
    /// Wrap a database handle. `fidelity` discloses the rebuild guarantee for
    /// the active vault mode (Loro store present → [`HistoryFidelity::Loro`]).
    /// The schema is created lazily on first use (so construction stays sync).
    pub fn new(db: DbHandle, fidelity: HistoryFidelity) -> Self {
        Self {
            db,
            fidelity,
            schema: OnceCell::new(),
        }
    }

    async fn ensure_schema(&self) -> Result<()> {
        self.schema
            .get_or_try_init(|| async {
                self.db
                    .execute_ddl(CREATE_TABLE_SQL)
                    .await
                    .context("creating block_history table")?;
                for ddl in [CREATE_INDEX_BLOCK, CREATE_INDEX_SESSION, CREATE_INDEX_AT] {
                    self.db
                        .execute_ddl(ddl)
                        .await
                        .context("creating block_history index")?;
                }
                anyhow::Ok(())
            })
            .await?;
        Ok(())
    }

    /// Build the `WHERE` clause + positional params for a filter. Kept together
    /// so `query` and `count` share exactly one predicate translation.
    fn where_clause(filter: &HistoryQuery) -> (String, Vec<Value>) {
        let mut clauses: Vec<&str> = Vec::new();
        let mut params: Vec<Value> = Vec::new();
        for (col, v) in [
            ("block_id = ?", &filter.block_id),
            ("origin = ?", &filter.origin),
            ("session_id = ?", &filter.session_id),
            ("field = ?", &filter.field),
            ("new_value = ?", &filter.new_value),
        ] {
            if let Some(s) = v {
                clauses.push(col);
                params.push(Value::String(s.clone()));
            }
        }
        if let Some(since) = filter.since_millis {
            clauses.push("at_millis >= ?");
            params.push(Value::Integer(since));
        }
        if let Some(until) = filter.until_millis {
            clauses.push("at_millis < ?");
            params.push(Value::Integer(until));
        }
        let sql = if clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", clauses.join(" AND "))
        };
        (sql, params)
    }
}

/// Read a `TEXT` column as `Option<String>`; `NULL`/absent → `None`. Fails loud
/// on a non-text, non-null value (parse-don't-validate on the row shape).
fn opt_text(row: &holon_core::storage::types::StorageEntity, col: &str) -> Result<Option<String>> {
    match row.get(col) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(other) => anyhow::bail!("block_history.{col} expected TEXT/NULL, got {other:?}"),
    }
}

fn req_text(row: &holon_core::storage::types::StorageEntity, col: &str) -> Result<String> {
    match row.get(col) {
        Some(Value::String(s)) => Ok(s.clone()),
        other => anyhow::bail!("block_history.{col} expected TEXT, got {other:?}"),
    }
}

fn req_int(row: &holon_core::storage::types::StorageEntity, col: &str) -> Result<i64> {
    match row.get(col) {
        Some(Value::Integer(i)) => Ok(*i),
        other => anyhow::bail!("block_history.{col} expected INTEGER, got {other:?}"),
    }
}

fn row_to_event(row: &holon_core::storage::types::StorageEntity) -> Result<HistoryEvent> {
    Ok(HistoryEvent {
        block_id: req_text(row, "block_id")?,
        op_name: req_text(row, "op_name")?,
        origin: req_text(row, "origin")?,
        transition_id: opt_text(row, "transition_id")?,
        session_id: opt_text(row, "session_id")?,
        tool_call_id: opt_text(row, "tool_call_id")?,
        field: opt_text(row, "field")?,
        new_value: opt_text(row, "new_value")?,
        at_millis: req_int(row, "at_millis")?,
    })
}

#[async_trait]
impl HistoryStore for TursoHistoryStore {
    fn fidelity(&self) -> HistoryFidelity {
        self.fidelity
    }

    async fn record(&self, event: HistoryEvent) -> Result<()> {
        self.ensure_schema().await?;
        let params = vec![
            Value::String(event.block_id),
            Value::String(event.op_name),
            Value::String(event.origin),
            event
                .transition_id
                .map(Value::String)
                .unwrap_or(Value::Null),
            event.session_id.map(Value::String).unwrap_or(Value::Null),
            event.tool_call_id.map(Value::String).unwrap_or(Value::Null),
            event.field.map(Value::String).unwrap_or(Value::Null),
            event.new_value.map(Value::String).unwrap_or(Value::Null),
            Value::Integer(event.at_millis),
        ];
        self.db
            .execute_values(
                "INSERT INTO block_history (block_id, op_name, origin, transition_id, session_id, \
                 tool_call_id, field, new_value, at_millis) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params,
            )
            .await
            .context("recording block_history event")?;
        Ok(())
    }

    async fn query(&self, filter: &HistoryQuery) -> Result<Vec<HistoryEvent>> {
        self.ensure_schema().await?;
        let (where_sql, params) = Self::where_clause(filter);
        let sql = format!(
            "SELECT block_id, op_name, origin, transition_id, session_id, tool_call_id, field, \
             new_value, at_millis FROM block_history{where_sql} ORDER BY seq ASC"
        );
        let rows = self
            .db
            .query_positional(&sql, params.iter().map(value_to_turso).collect())
            .await
            .context("querying block_history")?;
        rows.iter().map(row_to_event).collect()
    }

    async fn count(&self, filter: &HistoryQuery) -> Result<u64> {
        self.ensure_schema().await?;
        let (where_sql, params) = Self::where_clause(filter);
        let sql = format!("SELECT COUNT(*) AS n FROM block_history{where_sql}");
        let rows = self
            .db
            .query_positional(&sql, params.iter().map(value_to_turso).collect())
            .await
            .context("counting block_history")?;
        let n = req_int(rows.first().context("COUNT(*) returned no row")?, "n")?;
        Ok(n as u64)
    }
}

/// Convert a `holon_api::Value` positional param into a `turso::Value`. Only
/// the scalar shapes the history relation stores are handled; anything else is
/// a programming error (fail loud rather than coerce).
fn value_to_turso(v: &Value) -> turso::Value {
    match v {
        Value::String(s) => turso::Value::Text(s.clone()),
        Value::Integer(i) => turso::Value::Integer(*i),
        Value::Null => turso::Value::Null,
        other => panic!("block_history param must be TEXT/INTEGER/NULL, got {other:?}"),
    }
}

/// The degraded [`HistoryStore`] for org-standalone vaults with no Turso query
/// substrate. Discloses loudly: a warning at construction, [`HistoryFidelity::
/// None`], `record` is a disclosed no-op (so provenance stamping's block writes
/// never fail for lack of a cache), and reads return a loud, disclosed error.
pub struct DegradedHistoryStore {
    reason: String,
}

impl DegradedHistoryStore {
    pub fn new() -> Self {
        let reason = "history relation unavailable: this vault has no Turso query substrate \
                      (org-standalone degraded mode). Provenance is still stamped on blocks — \
                      query the block `_provenance` property, or open the vault with a Turso/CRDT \
                      backend for the queryable history relation."
            .to_string();
        tracing::warn!(target: "holon.history", "{reason}");
        Self { reason }
    }
}

impl Default for DegradedHistoryStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HistoryStore for DegradedHistoryStore {
    fn fidelity(&self) -> HistoryFidelity {
        HistoryFidelity::None
    }

    async fn record(&self, _event: HistoryEvent) -> Result<()> {
        // Disclosed no-op: nothing to record into (no query substrate). The
        // construction warning is the disclosure; failing here would break the
        // op path for lack of an ephemeral cache.
        Ok(())
    }

    async fn query(&self, _filter: &HistoryQuery) -> Result<Vec<HistoryEvent>> {
        anyhow::bail!("{}", self.reason)
    }

    async fn count(&self, _filter: &HistoryQuery) -> Result<u64> {
        anyhow::bail!("{}", self.reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::turso::TursoBackend;

    async fn store() -> (TursoBackend, TursoHistoryStore) {
        let (backend, db) = TursoBackend::new_in_memory().await.unwrap();
        (backend, TursoHistoryStore::new(db, HistoryFidelity::Loro))
    }

    fn ev(
        block: &str,
        op: &str,
        field: Option<&str>,
        value: Option<&str>,
        at: i64,
    ) -> HistoryEvent {
        HistoryEvent {
            block_id: block.to_string(),
            op_name: op.to_string(),
            origin: "rule".to_string(),
            transition_id: Some("rule:postpone".to_string()),
            session_id: None,
            tool_call_id: None,
            field: field.map(str::to_string),
            new_value: value.map(str::to_string),
            at_millis: at,
        }
    }

    #[tokio::test]
    async fn accrues_and_counts_postponements() {
        let (_backend, store) = store().await;
        // Block A postponed 3×, done 1×; block B postponed 1×.
        store
            .record(ev("A", "set_field", Some("status"), Some("postponed"), 10))
            .await
            .unwrap();
        store
            .record(ev("A", "set_field", Some("status"), Some("postponed"), 20))
            .await
            .unwrap();
        store
            .record(ev("A", "set_field", Some("status"), Some("done"), 30))
            .await
            .unwrap();
        store
            .record(ev("A", "set_field", Some("status"), Some("postponed"), 40))
            .await
            .unwrap();
        store
            .record(ev("B", "set_field", Some("status"), Some("postponed"), 50))
            .await
            .unwrap();

        let postponed_a = store
            .count(&HistoryQuery::transitions_to("A", "status", "postponed"))
            .await
            .unwrap();
        assert_eq!(postponed_a, 3, "block A was postponed 3 times");

        let all_a = store.query(&HistoryQuery::for_block("A")).await.unwrap();
        assert_eq!(all_a.len(), 4);
        // Ordered by seq (append order).
        assert_eq!(all_a[0].at_millis, 10);
        assert_eq!(all_a[3].new_value.as_deref(), Some("postponed"));
    }

    #[tokio::test]
    async fn queries_by_time_range() {
        let (_backend, store) = store().await;
        for at in [5, 15, 25, 35] {
            store
                .record(ev("A", "set_field", Some("status"), Some("x"), at))
                .await
                .unwrap();
        }
        let mid = HistoryQuery {
            since_millis: Some(15),
            until_millis: Some(35),
            ..Default::default()
        };
        let rows = store.query(&mid).await.unwrap();
        assert_eq!(rows.len(), 2, "15 <= at < 35 selects 15 and 25");
    }

    #[tokio::test]
    async fn rebuild_from_stream_reproduces_relation() {
        // Ephemerality proof: the relation is a pure function of the event
        // stream. Record a stream, snapshot the answers, then replay the SAME
        // stream into a FRESH store and assert identical answers.
        let events = vec![
            ev("A", "create", None, None, 1),
            ev("A", "set_field", Some("status"), Some("postponed"), 2),
            ev("B", "set_field", Some("status"), Some("postponed"), 3),
            ev("A", "set_field", Some("status"), Some("postponed"), 4),
        ];

        let (_b1, original) = store().await;
        for e in &events {
            original.record(e.clone()).await.unwrap();
        }
        let original_a = original.query(&HistoryQuery::for_block("A")).await.unwrap();
        let original_count = original
            .count(&HistoryQuery::transitions_to("A", "status", "postponed"))
            .await
            .unwrap();

        let (_b2, rebuilt) = store().await;
        for e in &events {
            rebuilt.record(e.clone()).await.unwrap();
        }
        let rebuilt_a = rebuilt.query(&HistoryQuery::for_block("A")).await.unwrap();
        let rebuilt_count = rebuilt
            .count(&HistoryQuery::transitions_to("A", "status", "postponed"))
            .await
            .unwrap();

        assert_eq!(original_a, rebuilt_a, "rebuilt relation matches original");
        assert_eq!(original_count, rebuilt_count);
        assert_eq!(rebuilt_count, 2);
    }

    #[tokio::test]
    async fn degraded_store_is_loud() {
        let degraded = DegradedHistoryStore::new();
        assert_eq!(degraded.fidelity(), HistoryFidelity::None);
        // record is a disclosed no-op (must not break the op path).
        degraded
            .record(ev("A", "create", None, None, 1))
            .await
            .unwrap();
        // reads fail loud with a disclosed reason.
        let err = degraded
            .query(&HistoryQuery::for_block("A"))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("org-standalone degraded mode"), "got: {err}");
        assert!(err.contains("_provenance"), "discloses the fallback: {err}");
        let count_err = degraded
            .count(&HistoryQuery::default())
            .await
            .unwrap_err()
            .to_string();
        assert!(count_err.contains("history relation unavailable"));
    }
}
