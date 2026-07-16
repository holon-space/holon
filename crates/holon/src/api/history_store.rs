//! [`HistoryStore`] implementations (VisionGapAnalysis C2b, ADR 0024 P8).
//!
//! - [`TursoHistoryStore`] — the full path: a plain `block_history` SQL table,
//!   maintained from the op/effect stream, queryable typed *and* joinable
//!   directly by matviews/PRQL (Martin's ruling allows the SQL surface). A
//!   disclosed ephemeral cache: rebuildable, never authoritative (Layer 3/4).
//!   The table's DDL is owned by `HistorySchemaModule` (holon-turso) and runs
//!   at boot — this type is only the typed accessor and fails loud if the
//!   schema module did not run.
//! - [`DegradedHistoryStore`] — org-standalone vaults with no Turso query
//!   substrate. Reads fail loud with a disclosed reason; `record_batch` is a
//!   disclosed no-op. Mirrors the CRDT-vs-LWW capability split.

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use holon_api::HistoryEvent;
use holon_api::HistoryFidelity;
use holon_api::HistoryQuery;
use holon_api::HistoryStore;
use holon_api::Value;
use holon_api::history::utc_day;
use tokio::sync::OnceCell;

use crate::storage::DbHandle;

/// A Turso-projected [`HistoryStore`]. The relation is a real SQL table so it
/// is directly joinable; this type is the thin typed accessor over it.
pub struct TursoHistoryStore {
    db: DbHandle,
    fidelity: HistoryFidelity,
    /// Next `op_group` to assign, seeded once from `MAX(op_group)` in the
    /// table — a deterministic monotonic sequence (pure function of table
    /// state and call order, never random, so PBT replay and
    /// rebuild-from-stream stay deterministic) that is unique across engine
    /// restarts (a session-scoped counter would collide after a restart).
    next_group: OnceCell<std::sync::atomic::AtomicI64>,
}

const INSERT_SQL: &str = "INSERT INTO block_history (entity_name, block_id, \
     op_name, origin, transition_id, session_id, tool_call_id, effect_id, \
     field, old_value, new_value, at_millis, day, op_group) \
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

const SELECT_COLS: &str = "entity_name, block_id, op_name, origin, \
     transition_id, session_id, tool_call_id, effect_id, field, old_value, \
     new_value, at_millis, op_group";

impl TursoHistoryStore {
    /// Wrap a database handle. `fidelity` discloses the rebuild guarantee for
    /// the active vault mode (Loro store present → [`HistoryFidelity::Loro`]).
    /// The `block_history` table itself is created at boot by
    /// `HistorySchemaModule`; this accessor fails loud if it is absent.
    pub fn new(db: DbHandle, fidelity: HistoryFidelity) -> Self {
        Self {
            db,
            fidelity,
            next_group: OnceCell::new(),
        }
    }

    /// The next fresh `op_group`, atomically. Seeded lazily from the table so
    /// groups stay unique across restarts and deterministic under replay.
    async fn next_op_group(&self) -> Result<i64> {
        let counter = self
            .next_group
            .get_or_try_init(|| async {
                let rows = self
                    .db
                    .query_positional(
                        "SELECT COALESCE(MAX(op_group), 0) AS g FROM block_history",
                        vec![],
                    )
                    .await
                    .context("seeding block_history op_group sequence")?;
                let max = req_int(rows.first().context("MAX(op_group) returned no row")?, "g")?;
                anyhow::Ok(std::sync::atomic::AtomicI64::new(max + 1))
            })
            .await?;
        Ok(counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst))
    }

    /// Build the `WHERE` clause + positional params for a filter. Kept together
    /// so `query` and `count` share exactly one predicate translation.
    fn where_clause(filter: &HistoryQuery) -> (String, Vec<Value>) {
        let mut clauses: Vec<&str> = Vec::new();
        let mut params: Vec<Value> = Vec::new();
        for (col, v) in [
            ("entity_name = ?", &filter.entity_name),
            ("block_id = ?", &filter.block_id),
            ("origin = ?", &filter.origin),
            ("session_id = ?", &filter.session_id),
            ("field = ?", &filter.field),
            ("new_value = ?", &filter.new_value),
            ("day = ?", &filter.day),
        ] {
            if let Some(s) = v {
                clauses.push(col);
                params.push(Value::String(s.clone()));
            }
        }
        if let Some(group) = filter.op_group {
            clauses.push("op_group = ?");
            params.push(Value::Integer(group));
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
        entity_name: req_text(row, "entity_name")?,
        block_id: req_text(row, "block_id")?,
        op_name: req_text(row, "op_name")?,
        origin: req_text(row, "origin")?,
        transition_id: opt_text(row, "transition_id")?,
        session_id: opt_text(row, "session_id")?,
        tool_call_id: opt_text(row, "tool_call_id")?,
        effect_id: opt_text(row, "effect_id")?,
        field: opt_text(row, "field")?,
        old_value: opt_text(row, "old_value")?,
        new_value: opt_text(row, "new_value")?,
        at_millis: req_int(row, "at_millis")?,
        op_group: Some(req_int(row, "op_group")?),
    })
}

/// The positional params for one event row, in [`INSERT_SQL`] column order.
/// `day` is derived here from `at_millis` (UTC, disclosed — see
/// [`holon_api::history::utc_day`]) so it can never drift from the timestamp.
fn insert_params(event: HistoryEvent, op_group: i64) -> Vec<turso::Value> {
    let day = utc_day(event.at_millis);
    let opt = |o: Option<String>| o.map(turso::Value::Text).unwrap_or(turso::Value::Null);
    vec![
        turso::Value::Text(event.entity_name),
        turso::Value::Text(event.block_id),
        turso::Value::Text(event.op_name),
        turso::Value::Text(event.origin),
        opt(event.transition_id),
        opt(event.session_id),
        opt(event.tool_call_id),
        opt(event.effect_id),
        opt(event.field),
        opt(event.old_value),
        opt(event.new_value),
        turso::Value::Integer(event.at_millis),
        turso::Value::Text(day),
        turso::Value::Integer(op_group),
    ]
}

#[async_trait]
impl HistoryStore for TursoHistoryStore {
    fn fidelity(&self) -> HistoryFidelity {
        self.fidelity
    }

    async fn record_batch(&self, events: Vec<HistoryEvent>) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        let op_group = self.next_op_group().await?;
        let statements: Vec<(String, Vec<turso::Value>)> = events
            .into_iter()
            .map(|event| (INSERT_SQL.to_string(), insert_params(event, op_group)))
            .collect();
        self.db
            .transaction(statements)
            .await
            .context("recording block_history event batch")?;
        Ok(())
    }

    async fn query(&self, filter: &HistoryQuery) -> Result<Vec<HistoryEvent>> {
        let (where_sql, params) = Self::where_clause(filter);
        let sql = format!("SELECT {SELECT_COLS} FROM block_history{where_sql} ORDER BY seq ASC");
        let rows = self
            .db
            .query_positional(&sql, params.iter().map(value_to_turso).collect())
            .await
            .context("querying block_history")?;
        rows.iter().map(row_to_event).collect()
    }

    async fn count(&self, filter: &HistoryQuery) -> Result<u64> {
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
/// None`], `record_batch` is a disclosed no-op (so provenance stamping's block
/// writes never fail for lack of a cache), and reads return a loud, disclosed
/// error.
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

    async fn record_batch(&self, _: Vec<HistoryEvent>) -> Result<()> {
        // Disclosed no-op: nothing to record into (no query substrate). The
        // construction warning is the disclosure; failing here would break the
        // op path for lack of an ephemeral cache.
        Ok(())
    }

    async fn query(&self, _: &HistoryQuery) -> Result<Vec<HistoryEvent>> {
        anyhow::bail!("{}", self.reason)
    }

    async fn count(&self, _: &HistoryQuery) -> Result<u64> {
        anyhow::bail!("{}", self.reason)
    }
}

#[cfg(test)]
mod tests {
    use holon_turso::schema_module::SchemaModule;
    use holon_turso::schema_modules::HistorySchemaModule;

    use super::*;
    use crate::storage::turso::TursoBackend;

    async fn store() -> (TursoBackend, TursoHistoryStore) {
        let (backend, db) = TursoBackend::new_in_memory().await.unwrap();
        HistorySchemaModule.ensure_schema(&db).await.unwrap();
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
            entity_name: "block".to_string(),
            block_id: block.to_string(),
            op_name: op.to_string(),
            origin: "rule".to_string(),
            transition_id: Some("rule:postpone".to_string()),
            session_id: None,
            tool_call_id: None,
            effect_id: None,
            field: field.map(str::to_string),
            old_value: None,
            new_value: value.map(str::to_string),
            at_millis: at,
            op_group: None,
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
    async fn batch_shares_one_op_group_and_groups_are_distinct() {
        let (_backend, store) = store().await;
        // One op touching two fields: one batch, one group.
        store
            .record_batch(vec![
                ev("A", "update", Some("status"), Some("doing"), 10),
                ev("A", "update", Some("title"), Some("x"), 10),
            ])
            .await
            .unwrap();
        // A second op: a fresh group.
        store
            .record(ev("A", "set_field", Some("status"), Some("done"), 20))
            .await
            .unwrap();

        let all = store.query(&HistoryQuery::for_block("A")).await.unwrap();
        assert_eq!(all.len(), 3);
        let g0 = all[0].op_group.expect("read-back rows carry op_group");
        assert_eq!(all[1].op_group, Some(g0), "one op = one group");
        let g2 = all[2].op_group.unwrap();
        assert_ne!(g2, g0, "distinct ops get distinct groups");

        // The group filter selects exactly the op's rows.
        let group_rows = store
            .query(&HistoryQuery {
                op_group: Some(g0),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(group_rows.len(), 2);
    }

    #[tokio::test]
    async fn op_group_unique_across_store_reopen() {
        // Two accessors over the SAME db (an engine restart): the second must
        // seed past the first's groups, never reuse them.
        let (_backend, first) = store().await;
        first
            .record(ev("A", "create", None, Some("x"), 1))
            .await
            .unwrap();
        first
            .record(ev("A", "set_field", Some("s"), Some("y"), 2))
            .await
            .unwrap();

        let reopened = TursoHistoryStore::new(first.db.clone(), HistoryFidelity::Loro);
        reopened
            .record(ev("A", "set_field", Some("s"), Some("z"), 3))
            .await
            .unwrap();

        let all = reopened.query(&HistoryQuery::default()).await.unwrap();
        let mut groups: Vec<i64> = all.iter().map(|e| e.op_group.unwrap()).collect();
        let before_dedup = groups.len();
        groups.dedup();
        assert_eq!(groups.len(), before_dedup, "no op_group reuse: {groups:?}");
        assert!(
            groups.windows(2).all(|w| w[0] < w[1]),
            "monotonic: {groups:?}"
        );
    }

    #[tokio::test]
    async fn day_column_is_derived_and_queryable() {
        let (_backend, store) = store().await;
        // Two events a day apart; the day filter must separate them.
        let noon = 1_784_203_200_000_i64;
        store
            .record(ev("A", "create", None, None, noon))
            .await
            .unwrap();
        store
            .record(ev("B", "create", None, None, noon + 86_400_000))
            .await
            .unwrap();
        let day = holon_api::history::utc_day(noon);
        let rows = store
            .query(&HistoryQuery {
                day: Some(day.clone()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "day {day} selects only the first event");
        assert_eq!(rows[0].block_id, "A");
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
    async fn stale_table_shape_is_dropped_and_recreated() {
        // A pre-op_group (schema v1) table must be replaced at boot, not
        // migrated — the relation is a disclosed ephemeral cache.
        let (_backend, db) = TursoBackend::new_in_memory().await.unwrap();
        db.execute_ddl(
            "CREATE TABLE block_history (seq INTEGER PRIMARY KEY, \
             block_id TEXT NOT NULL, at_millis INTEGER NOT NULL)",
        )
        .await
        .unwrap();
        HistorySchemaModule.ensure_schema(&db).await.unwrap();
        // The v2 accessor works against the recreated table.
        let store = TursoHistoryStore::new(db, HistoryFidelity::Loro);
        store
            .record(ev("A", "create", None, None, 1))
            .await
            .unwrap();
        assert_eq!(store.count(&HistoryQuery::default()).await.unwrap(), 1);
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
        assert!(
            err.contains("_provenance"),
            "discloses the block stamp: {err}"
        );
        let count_err = degraded
            .count(&HistoryQuery::default())
            .await
            .unwrap_err()
            .to_string();
        assert!(count_err.contains("history relation unavailable"));
    }
}
