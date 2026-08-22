//! Manages materialized view lifecycle — creation, existence checks,
//! orphan cleanup, CDC subscription, and querying.
//!
//! Consolidates the matview lifecycle that was previously duplicated across
//! `BackendEngine::preload_views`, `BackendEngine::watch_query`, and
//! `WatchedQuery::new`.

use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use anyhow::Context;
use anyhow::Result;
use holon_api::BatchWithMetadata;
use holon_api::Value;
use holon_core::storage::Resource;
use holon_core::storage::StorageEntity;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

use crate::sql_parser::extract_table_refs;
use crate::sql_parser::parse_sql;
use crate::turso::DbHandle;
use crate::turso::RowChange;
use crate::turso::RowChangeStream;
use crate::turso::priority;
use crate::util::strip_order_by;

/// Normalize a SQL statement for comparison: collapse whitespace, strip
/// trailing semicolons, lowercase keywords, and drop spaces before `(` (Turso's
/// view pretty-printer emits `iif (` / `strftime (`). This lets us compare
/// `sqlite_master.sql` against the desired CREATE statement without false
/// positives from formatting differences.
fn normalize_view_sql(sql: &str) -> String {
    sql.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches(';')
        .to_lowercase()
        .replace(" (", "(")
}

/// Reconcile a named materialized view: only DROP+CREATE if the SELECT changed.
///
/// Accepts just the SELECT query (like `ensure_view` does for dynamic views)
/// and constructs the full `CREATE MATERIALIZED VIEW {name} AS {select}`
/// itself. Compares against `sqlite_master.sql` to detect changes.
///
/// This is a free function taking `DbHandle` so it can be called from
/// `SchemaModule` implementations that don't have access to `MatviewManager`.
///
/// Returns `true` if the view was (re)created, `false` if it already matched.
pub async fn reconcile_named_view(
    db_handle: &DbHandle,
    view_name: &str,
    select_sql: &str,
) -> Result<bool> {
    let create_sql = format!("CREATE MATERIALIZED VIEW {} AS {}", view_name, select_sql);

    // A base table declaring `ON CONFLICT REPLACE` gives every later PLAIN
    // INSERT full REPLACE semantics, which on this fork silently drops rows
    // from every view over it. The DDL screen refuses that clause going
    // forward; this covers the tables that already carry it — created before
    // the screen existed, or outside DbHandle entirely — by refusing to build a
    // view on top of one. Registration is the last point where the pairing can
    // still be seen.
    reject_on_conflict_replace_bases(db_handle, view_name, select_sql).await?;

    let rows = db_handle
        .query(
            &format!(
                "SELECT sql FROM sqlite_master WHERE type='view' AND name='{}'",
                view_name
            ),
            HashMap::new(),
        )
        .await?;

    if let Some(row) = rows.first() {
        if let Some(Value::String(existing_sql)) = row.get("sql") {
            if normalize_view_sql(existing_sql) == normalize_view_sql(&create_sql) {
                tracing::debug!(
                    "[reconcile_named_view] View '{}' unchanged, skipping",
                    view_name
                );
                return Ok(false);
            }
            tracing::info!(
                "[reconcile_named_view] View '{}' definition changed, recreating",
                view_name
            );
        }
        // Recreating a matview that other matviews chain on silently CORRUPTS
        // the dependents (they keep their old rows AND receive the recreated
        // base's rows as fresh inserts — duplicate rows; pinned by
        // holon-advice/tests/matview_build.rs::probe_multi_junction_fanout_fix_shapes).
        // Cascade-drop the dependents first; their owning schema modules run
        // AFTER this one (that is the dependency direction) and recreate them
        // fresh, and dynamic watch views are recreated on watch registration.
        drop_dependent_views(db_handle, view_name).await?;
        db_handle
            .execute_ddl(&format!("DROP VIEW IF EXISTS {}", view_name))
            .await?;
    }

    // Idempotent-or-recovering create. The `sqlite_master` probe above only sees
    // rows of `type='view'`; a crash mid-DDL can leave the matview's backing
    // table and/or its `__turso_internal_dbsp_state_*` tables behind WITHOUT a
    // committed view row. A plain `CREATE MATERIALIZED VIEW` then dies with
    // "table <name> already exists" and the app boot-loops (on Android the only
    // escape was `pm clear` = user data loss). Recover instead: `CREATE ... IF
    // NOT EXISTS` — Turso's create path itself reclaims a current-epoch orphaned
    // DBSP state table — and on a residual name collision drop the orphaned
    // *view-named* backing object (NOT the system-prefixed DBSP table, which
    // Turso forbids dropping) and retry ONCE. `cleanup_orphaned_dbsp_state`
    // only DISCLOSES any surviving DBSP residue; it never issues the forbidden
    // `DROP TABLE __turso_internal_*` that was itself the Android boot panic.
    // This is data-safe: a matview is a pure projection over its base tables
    // (e.g. `block` over `block_raw`), which are never touched here, so the
    // source of truth survives. If the retry still fails we surface a clear
    // error rather than looping forever.
    cleanup_orphaned_dbsp_state(db_handle, view_name).await?;
    let create_idempotent = format!(
        "CREATE MATERIALIZED VIEW IF NOT EXISTS {} AS {}",
        view_name, select_sql
    );
    match db_handle.execute_ddl(&create_idempotent).await {
        Ok(()) => {
            tracing::info!(
                "[reconcile_named_view] View '{}' created/updated",
                view_name
            );
            Ok(true)
        }
        Err(e) if e.to_string().contains("already exists") => {
            tracing::warn!(
                "[reconcile_named_view] View '{view_name}' collided with orphaned backing objects \
                 left by a prior crash ({e}); dropping the derived matview + orphaned DBSP state \
                 and recreating (base tables are untouched — no data loss)"
            );
            drop_dependent_views(db_handle, view_name).await?;
            db_handle
                .execute_ddl(&format!("DROP VIEW IF EXISTS {}", view_name))
                .await?;
            db_handle
                .execute_ddl(&format!("DROP TABLE IF EXISTS {}", view_name))
                .await?;
            cleanup_orphaned_dbsp_state(db_handle, view_name).await?;
            db_handle
                .execute_ddl(&create_idempotent)
                .await
                .map_err(|e2| {
                    anyhow::anyhow!(
                        "reconcile_named_view: matview '{view_name}' could not be (re)created \
                         even after clearing orphaned backing objects left by a prior crash: \
                         {e2}. The schema is genuinely incompatible; failing loudly instead of \
                         boot-looping. Base tables are intact — inspect the DB rather than \
                         clearing user data."
                    )
                })?;
            tracing::info!(
                "[reconcile_named_view] View '{}' recovered and recreated",
                view_name
            );
            Ok(true)
        }
        Err(e) => Err(e.into()),
    }
}

/// Every dynamic watch matview is named `watch_view_{hash-of-its-SELECT}`.
pub const WATCH_VIEW_PREFIX: &str = "watch_view_";

/// Whether the fork's DBSP IVM can correctly maintain a matview for `sql`.
///
/// The DBSP incremental engine cannot maintain a **subquery-valued predicate**
/// — `EXISTS`, `NOT EXISTS`, or `IN (subquery)` / `NOT IN (subquery)` — the
/// `Exists` operator itself, negated or not (`Cannot convert LogicalExpr to AST
/// Expr: Exists { … negated: false/true }`).
///
/// Whether that refusal is LOUD or SILENT depends not on chaining but on what
/// sits BESIDE the subquery in `WHERE` (turso-6f 8-shape bisect, corroborated
/// by this lane's own probe D/E):
///   * a plain-column conjunct beside the subquery (`b.id <> 'x' AND NOT EXISTS
///     (…)`) → refused LOUDLY at DDL.
///   * a COMPUTED conjunct beside it (Now.org's leading
///     `json_extract(properties,'$.task_state')='TODO' AND NOT EXISTS (…)`) →
///     the projection rewrite's catch-all aliased the subquery onto the shared
///     `__temp_filter_expr` temp column, so CREATE SILENTLY SUCCEEDED with an
///     always-false compiled filter (0 rows) while a fresh recompute returns
///     the real set (bugfunnel 2026-08-19-ivm-antijoin-matview-silently-empty).
/// The silent case is the fail-loud violation. The turso engine fix (validate
/// every substituted sub-expression through the conversion authority, no
/// allowlist) makes unsupported shapes refuse LOUDLY in ALL combinations; this
/// predicate stays correct under that fix (it routes every subquery-in-`WHERE`
/// shape eager regardless), so such a query is never served from a matview —
/// the caller serves it by eager re-execution in a disclosed degraded mode.
///
/// The decision is **inverted to engine truth**: route to a matview ONLY when
/// the shape is provably maintainable. A raw-SQL substring test cannot do this
/// — it misses the `NOT (EXISTS …)` spelling the keystone's own generator emits
/// and false-flags a `'… not exists'` string literal. So we PARSE and VISIT the
/// AST NODES (not its Debug text): a query is un-maintainable iff its parse
/// tree contains an `Exists` or `InSubquery` expression anywhere (any nesting,
/// any spelling). Because we match on the typed node — never on rendered text —
/// a string literal like `content = 'Exists {'` cannot trip the check. A parse
/// failure is conservatively un-maintainable (eager always serves correct
/// rows), disclosed by a warning.
///
/// This is a subset filter, not a full capability oracle: it names the WHERE
/// subquery-predicate shapes we KNOW the fork mis-handles, to skip a doomed
/// CREATE. Anything it lets through that the engine still refuses (a scalar
/// subquery in the SELECT list, `EXCEPT`/`INTERSECT`, a derived table) is
/// caught at CREATE by the permanent-refusal backstop in `query_and_watch` and
/// served eager — so a false-negative here degrades gracefully, never wrongly.
pub fn sql_ivm_maintainable(sql: &str) -> bool {
    match crate::sql_parser::parse_sql(sql) {
        Ok(statements) => !statements.iter().any(statement_has_subquery_predicate),
        Err(e) => {
            tracing::warn!(
                sql = %sql.chars().take(160).collect::<String>(),
                "sql_ivm_maintainable: SQL did not parse — routing to eager re-execution \
                 (conservative; eager always serves correct rows): {e}"
            );
            false
        }
    }
}

/// Does any expression anywhere under `stmt` use `EXISTS` or `IN (subquery)`?
fn statement_has_subquery_predicate(stmt: &sqlparser::ast::Statement) -> bool {
    match stmt {
        sqlparser::ast::Statement::Query(q) => query_has_subquery_predicate(q),
        _ => false,
    }
}

fn query_has_subquery_predicate(q: &sqlparser::ast::Query) -> bool {
    use sqlparser::ast::SetExpr;
    if let Some(with) = &q.with {
        if with
            .cte_tables
            .iter()
            .any(|c| query_has_subquery_predicate(&c.query))
        {
            return true;
        }
    }
    fn setexpr(body: &SetExpr) -> bool {
        match body {
            SetExpr::Select(s) => select_has_subquery_predicate(s),
            SetExpr::Query(q) => query_has_subquery_predicate(q),
            SetExpr::SetOperation { left, right, .. } => setexpr(left) || setexpr(right),
            _ => false,
        }
    }
    setexpr(&q.body)
}

fn select_has_subquery_predicate(s: &sqlparser::ast::Select) -> bool {
    use sqlparser::ast::GroupByExpr;
    use sqlparser::ast::SelectItem;
    use sqlparser::ast::TableFactor;
    let projection = s.projection.iter().any(|item| match item {
        SelectItem::UnnamedExpr(e) => expr_has_subquery(e),
        SelectItem::ExprWithAlias { expr, .. } => expr_has_subquery(expr),
        _ => false,
    });
    let selection = s.selection.as_ref().is_some_and(expr_has_subquery);
    let having = s.having.as_ref().is_some_and(expr_has_subquery);
    let group_by = match &s.group_by {
        GroupByExpr::Expressions(exprs, _) => exprs.iter().any(expr_has_subquery),
        _ => false,
    };
    let from = s.from.iter().any(|twj| {
        let derived = |tf: &TableFactor| {
            matches!(tf, TableFactor::Derived { subquery, .. } if query_has_subquery_predicate(subquery))
        };
        let joins = twj.joins.iter().any(|j| {
            derived(&j.relation)
                || join_on_expr(&j.join_operator).is_some_and(expr_has_subquery)
        });
        derived(&twj.relation) || joins
    });
    projection || selection || having || group_by || from
}

/// The load-bearing node check: an `Exists` / `InSubquery` expression, at any
/// nesting depth reachable through the common compound operators. A shape this
/// misses is caught by the CREATE-time backstop, so `_ => false` on an exotic
/// leaf is safe, not silent-wrong.
fn expr_has_subquery(e: &sqlparser::ast::Expr) -> bool {
    use sqlparser::ast::Expr;
    let recur = expr_has_subquery;
    match e {
        Expr::Exists { .. } | Expr::InSubquery { .. } => true,
        Expr::Subquery(q) => query_has_subquery_predicate(q),
        Expr::BinaryOp { left, right, .. } => recur(left) || recur(right),
        Expr::UnaryOp { expr, .. } => recur(expr),
        Expr::Nested(x) => recur(x),
        Expr::IsNull(x)
        | Expr::IsNotNull(x)
        | Expr::IsTrue(x)
        | Expr::IsNotTrue(x)
        | Expr::IsFalse(x)
        | Expr::IsNotFalse(x)
        | Expr::IsUnknown(x)
        | Expr::IsNotUnknown(x) => recur(x),
        Expr::Between {
            expr, low, high, ..
        } => recur(expr) || recur(low) || recur(high),
        Expr::InList { expr, list, .. } => recur(expr) || list.iter().any(recur),
        Expr::Like { expr, pattern, .. }
        | Expr::ILike { expr, pattern, .. }
        | Expr::SimilarTo { expr, pattern, .. } => recur(expr) || recur(pattern),
        Expr::Cast { expr, .. } | Expr::Collate { expr, .. } => recur(expr),
        Expr::Case {
            operand,
            conditions,
            else_result,
            ..
        } => {
            operand.as_deref().is_some_and(recur)
                || conditions
                    .iter()
                    .any(|w| recur(&w.condition) || recur(&w.result))
                || else_result.as_deref().is_some_and(recur)
        }
        Expr::Tuple(xs) => xs.iter().any(recur),
        _ => false,
    }
}

/// The `ON` expression of a join, for the common constraint-bearing operators.
/// Missing an operator only forfeits an early eager route — the CREATE-time
/// backstop still catches an un-maintainable join.
fn join_on_expr(op: &sqlparser::ast::JoinOperator) -> Option<&sqlparser::ast::Expr> {
    use sqlparser::ast::JoinConstraint;
    use sqlparser::ast::JoinOperator;
    let constraint = match op {
        JoinOperator::Inner(c)
        | JoinOperator::LeftOuter(c)
        | JoinOperator::RightOuter(c)
        | JoinOperator::FullOuter(c)
        | JoinOperator::Join(c) => c,
        _ => return None,
    };
    match constraint {
        JoinConstraint::On(e) => Some(e),
        _ => None,
    }
}

/// True when `sql` references `name` as a standalone identifier token
/// (`block` matches `FROM block b` but not `block_raw`).
fn sql_references_identifier(sql: &str, name: &str) -> bool {
    sql.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(|tok| tok.eq_ignore_ascii_case(name))
}

/// Recursively drop every view/matview whose definition references
/// `view_name`, depth-first (dependents-of-dependents go first). Used before
/// DROP+recreate of a changed matview: leaving dependents in place corrupts
/// them with duplicate rows (see the call site in [`reconcile_named_view`]).
///
/// `view_name` may equally be a BASE TABLE: a table carrying dependent matviews
/// cannot be `ALTER TABLE ... RENAME`d (Turso rejects it), so a shape migration
/// must clear them first and let the owning schema modules rebuild them on
/// their normal pass. See
/// `schema_modules::migrate_junction_dropping_target_fk`.
pub(crate) async fn drop_dependent_views(db_handle: &DbHandle, view_name: &str) -> Result<()> {
    let rows = db_handle
        .query(
            "SELECT name, sql FROM sqlite_master WHERE type='view'",
            HashMap::new(),
        )
        .await?;
    for row in rows {
        let (Some(Value::String(name)), Some(Value::String(sql))) =
            (row.get("name"), row.get("sql"))
        else {
            continue;
        };
        if name.eq_ignore_ascii_case(view_name) || !sql_references_identifier(sql, view_name) {
            continue;
        }
        tracing::warn!(
            "[reconcile_named_view] Dropping matview '{name}' because its base matview \
             '{view_name}' is being recreated; it will be rebuilt by its owning schema module (or \
             on watch registration) — leaving it in place would corrupt it with duplicate rows"
        );
        Box::pin(drop_dependent_views(db_handle, name)).await?;
        db_handle
            .execute_ddl(&format!("DROP VIEW IF EXISTS {name}"))
            .await?;
        cleanup_orphaned_dbsp_state(db_handle, name).await?;
    }
    Ok(())
}

/// Disclose (never DROP) `__turso_internal_dbsp_state_v*_{view_name}` tables
/// left behind by a crash mid-DDL or an older DBSP-circuit epoch.
///
/// These are Turso IVM internal state, NOT user data. The `__turso_internal_`
/// prefix is reserved: a user `DROP TABLE` on it bails `Cannot drop system
/// table ...` — which on Android surfaced as a stale-DB boot panic (this
/// function's previous `DROP TABLE` was itself the crash). Disposal is Turso's
/// own responsibility:
///   - `CREATE MATERIALIZED VIEW` cleans a **current**-version orphan as part
///     of (re)creation (see turso `translate_create_materialized_view`), so no
///     `already exists` collision survives the create;
///   - `DROP VIEW` cleans it when the matview row still exists.
///
/// A table matched here is therefore either about to be reclaimed by the
/// following CREATE, or an **older**-epoch residue whose name carries a
/// different circuit version so the current CREATE never collides with it.
/// We surface it loudly (fail-loud disclosure) but leave disposal to Turso;
/// base tables — the source of truth — are never touched, so no data loss.
async fn cleanup_orphaned_dbsp_state(db_handle: &DbHandle, view_name: &str) -> Result<()> {
    let pattern = format!("__turso_internal_dbsp_state_v%_{}", view_name);
    let check_sql = format!(
        "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '{}'",
        pattern
    );
    let residual = db_handle.query(&check_sql, HashMap::new()).await?;
    for row in residual {
        if let Some(Value::String(table_name)) = row.get("name") {
            tracing::warn!(
                "[reconcile_named_view] Leaving Turso-internal DBSP state table '{table_name}' \
                 in place: its reserved prefix forbids a user DROP, and disposal belongs to \
                 Turso's own CREATE/DROP MATERIALIZED VIEW path (current-epoch state is \
                 reclaimed by the following CREATE; an older-epoch residue does not collide). \
                 Base tables are untouched — no data loss."
            );
        }
    }
    Ok(())
}

// --- Connection-scoped siblings, for callers running INSIDE the database actor
//
// The `DbHandle` variants above enqueue a command; a caller that is *itself* a
// command being processed would then wait for a queue it is blocking —
// self-deadlock. These take the actor's own connection instead.

/// Names of every view/matview whose definition references `view_name`,
/// deepest dependent first, so dropping the list in order never leaves a view
/// standing on a dropped base.
///
/// The actor collects rather than drops so the reap policy (refuse to reap a
/// dependent that still holds live leases) stays where the lease state lives.
pub(crate) async fn dependent_views_on_conn(
    conn: &turso::Connection,
    view_name: &str,
) -> Result<Vec<String>> {
    let rows = crate::turso::TursoBackend::handle_query(
        conn,
        "SELECT name, sql FROM sqlite_master WHERE type='view'",
        HashMap::new(),
    )
    .await?;

    let mut ordered = Vec::new();
    for row in rows {
        let (Some(Value::String(name)), Some(Value::String(sql))) =
            (row.get("name"), row.get("sql"))
        else {
            continue;
        };
        if name.eq_ignore_ascii_case(view_name) || !sql_references_identifier(sql, view_name) {
            continue;
        }
        ordered.extend(Box::pin(dependent_views_on_conn(conn, name)).await?);
        ordered.push(name.clone());
    }
    Ok(ordered)
}

/// Connection-scoped [`cleanup_orphaned_dbsp_state`] — disclose, never DROP.
/// See that function's doc for why disposal belongs to Turso.
pub(crate) async fn cleanup_orphaned_dbsp_state_on_conn(
    conn: &turso::Connection,
    view_name: &str,
) -> Result<()> {
    let check_sql = format!(
        "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE \
         '__turso_internal_dbsp_state_v%_{view_name}'"
    );
    let residual =
        crate::turso::TursoBackend::handle_query(conn, &check_sql, HashMap::new()).await?;
    for row in residual {
        if let Some(Value::String(table_name)) = row.get("name") {
            tracing::warn!(
                "[Actor] Leaving Turso-internal DBSP state table '{table_name}' in place: its \
                 reserved prefix forbids a user DROP, and disposal belongs to Turso's own \
                 CREATE/DROP MATERIALIZED VIEW path. Base tables are untouched — no data loss."
            );
        }
    }
    Ok(())
}

// MatviewHook (the FDW-primed callback) is a storage-agnostic trait; it lives
// in holon-core so providers implement it without naming the Turso backend.
use holon_core::MatviewHook;

/// Result of watching a query — initial data + CDC stream.
pub struct WatchResult {
    pub initial_rows: Vec<StorageEntity>,
    pub stream: RowChangeStream,
    pub view_name: String,
}

/// Command sent to the CDC demultiplexer task.
enum DemuxCommand {
    /// Register a new subscriber for a specific view. `ack` fires once the
    /// subscriber is registered, so callers can order registration before
    /// their initial query.
    Subscribe {
        view_name: String,
        tx: mpsc::Sender<BatchWithMetadata<RowChange>>,
        ack: oneshot::Sender<()>,
    },
}

/// Manages the full lifecycle of Turso materialized views.
///
/// CDC routing uses a single demultiplexer task instead of spawning one filter
/// task per `subscribe_cdc()` call. The demux task reads from the broadcast
/// channel and routes batches to registered subscribers by `relation_name`.
/// Closed subscribers are pruned automatically.
/// @c4 code
pub struct MatviewManager {
    db_handle: DbHandle,
    demux_cmd_tx: mpsc::Sender<DemuxCommand>,
    ddl_mutex: Arc<tokio::sync::Mutex<()>>,
    /// Cache tables that have an associated FDW table (`{name}_fdw`).
    fdw_backed_tables: Arc<tokio::sync::RwLock<HashSet<String>>>,
    /// Optional hook called after FDW cache priming.
    hook: Arc<tokio::sync::RwLock<Option<Arc<dyn MatviewHook>>>>,
    /// Cache of view names known to exist in `sqlite_master`, and the DDL
    /// mutex guarding create-if-absent. Both come from [`shared_for_database`],
    /// so every manager on one database sees one cache and one mutex.
    known_views: Arc<tokio::sync::RwLock<HashSet<String>>>,
    /// Reap epoch `known_views` was established under; see [`SharedViewState`].
    validated_at: Arc<AtomicU64>,
    /// Counters for measuring cache effectiveness. `cache_hits` is the number
    /// of `ensure_view`/`preload` calls that returned via the in-memory cache
    /// without a `view_exists` SQL round trip. `exists_calls` is the number of
    /// `view_exists` SQL round trips actually issued. `ddl_creates` counts
    /// successful CREATE MATERIALIZED VIEW executions.
    cache_hits: Arc<AtomicU64>,
    exists_calls: Arc<AtomicU64>,
    ddl_creates: Arc<AtomicU64>,
}

/// The view-existence cache and DDL mutex belonging to ONE database.
#[derive(Clone)]
struct SharedViewState {
    known_views: Arc<tokio::sync::RwLock<HashSet<String>>>,
    ddl_mutex: Arc<tokio::sync::Mutex<()>>,
    /// Reap epoch the cache's contents were established under. The actor drops
    /// views on its own schedule (`reap_view`), so a name learned before a reap
    /// says nothing about the schema now.
    validated_at: Arc<AtomicU64>,
}

/// Live databases' [`SharedViewState`], keyed by the address of the database's
/// witness and validated against the witness itself.
///
/// Keyed PER DATABASE, never per process: view names are content hashes of
/// their SELECT (`compute_view_name`), so the same name denotes different
/// views in different databases, and one process routinely holds several
/// (every test binary). A process-wide cache would let a view created in one
/// database suppress the `CREATE` in another, leaving a query reading a view
/// that does not exist.
static SHARED_VIEW_STATE: std::sync::Mutex<
    Option<HashMap<usize, (std::sync::Weak<AtomicU64>, SharedViewState)>>,
> = std::sync::Mutex::new(None);

/// The state every `MatviewManager` on `db_handle`'s database must share,
/// creating it on first use with `seed_mutex`.
///
/// A dead entry is replaced rather than reused: the key is an allocation
/// address, and a database that has been dropped can leave its address to a
/// new one.
fn shared_for_database(
    db_handle: &DbHandle,
    seed_mutex: Arc<tokio::sync::Mutex<()>>,
) -> SharedViewState {
    let witness = db_handle.database_witness();
    let key = witness.as_ptr() as usize;
    let mut guard = SHARED_VIEW_STATE
        .lock()
        .expect("SHARED_VIEW_STATE poisoned");
    let table = guard.get_or_insert_with(HashMap::new);
    if let Some((known_witness, state)) = table.get(&key)
        && known_witness.strong_count() > 0
        && std::sync::Weak::ptr_eq(known_witness, &witness)
    {
        return state.clone();
    }
    let state = SharedViewState {
        known_views: Arc::new(tokio::sync::RwLock::new(HashSet::new())),
        ddl_mutex: seed_mutex,
        validated_at: Arc::new(AtomicU64::new(db_handle.reap_epoch())),
    };
    table.insert(key, (witness, state.clone()));
    state
}

impl MatviewManager {
    /// `ddl_mutex` seeds the database's shared mutex if this is the first
    /// manager on it; a later manager adopts the one already there, so two
    /// managers can never serialise their DDL against different locks.
    pub fn new(db_handle: DbHandle, ddl_mutex: Arc<tokio::sync::Mutex<()>>) -> Self {
        let demux_cmd_tx = Self::spawn_demux(db_handle.cdc_broadcast().clone());
        let shared = shared_for_database(&db_handle, ddl_mutex);
        Self {
            db_handle,
            demux_cmd_tx,
            ddl_mutex: shared.ddl_mutex,
            fdw_backed_tables: Arc::new(tokio::sync::RwLock::new(HashSet::new())),
            hook: Arc::new(tokio::sync::RwLock::new(None)),
            known_views: shared.known_views,
            validated_at: shared.validated_at,
            cache_hits: Arc::new(AtomicU64::new(0)),
            exists_calls: Arc::new(AtomicU64::new(0)),
            ddl_creates: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Snapshot of (cache_hits, exists_calls, ddl_creates). Useful for tests
    /// and one-off profiling — the cache is a hot path so we keep counters
    /// in atomics even in release builds.
    pub fn cache_metrics(&self) -> (u64, u64, u64) {
        (
            self.cache_hits.load(Ordering::Relaxed),
            self.exists_calls.load(Ordering::Relaxed),
            self.ddl_creates.load(Ordering::Relaxed),
        )
    }

    /// Register a cache table as FDW-backed. Matview creation will auto-prime
    /// the cache from the FDW before building the view.
    pub async fn register_fdw_table(&self, cache_table: &str) {
        self.fdw_backed_tables
            .write()
            .await
            .insert(cache_table.to_string());
    }

    /// Set the hook called after successful FDW cache priming.
    pub async fn set_hook(&self, hook: Arc<dyn MatviewHook>) {
        *self.hook.write().await = Some(hook);
    }

    /// Spawn the single CDC demultiplexer task.
    ///
    /// Reads from the broadcast channel and fans out to per-view subscribers.
    /// Dead subscribers (closed channels) are pruned on each batch.
    fn spawn_demux(
        cdc_broadcast: broadcast::Sender<BatchWithMetadata<RowChange>>,
    ) -> mpsc::Sender<DemuxCommand> {
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<DemuxCommand>(64);
        let mut broadcast_rx = cdc_broadcast.subscribe();
        crate::util::spawn_actor(async move {
            let mut subscribers: HashMap<String, Vec<mpsc::Sender<BatchWithMetadata<RowChange>>>> =
                HashMap::new();
            let mut cmd_rx_open = true;

            loop {
                // Stop when no subscribers remain AND the command channel is closed
                // (no new subscribers can arrive)
                if !cmd_rx_open && subscribers.is_empty() {
                    break;
                }

                tokio::select! {
                    // Process new subscriber registrations (only when channel is open)
                    maybe_cmd = cmd_rx.recv(), if cmd_rx_open => {
                        match maybe_cmd {
                            Some(DemuxCommand::Subscribe { view_name, tx, ack }) => {
                                tracing::info!("[Demux] Registered subscriber for '{}'", view_name);
                                subscribers.entry(view_name).or_default().push(tx);
                                // Receiver gone = caller aborted before registration
                                // completed; nothing to notify.
                                let _ = ack.send(());
                            }
                            None => {
                                // MatviewManager dropped — stop accepting new subscribers
                                // but keep delivering to existing ones
                                cmd_rx_open = false;
                            }
                        }
                    }
                    // Route CDC batches to matching subscribers
                    result = broadcast_rx.recv() => {
                        match result {
                            Ok(batch) => {
                                let view_name = &batch.metadata.relation_name;
                                let sub_count = subscribers.get(view_name).map(|s| s.len()).unwrap_or(0);
                                if !batch.inner.items.is_empty() {
                                    if sub_count > 0 {
                                        tracing::debug!(
                                            "[Demux] view='{}' items={} subscribers={}",
                                            view_name, batch.inner.items.len(), sub_count
                                        );
                                    } else {
                                        tracing::trace!(
                                            "[Demux] view='{}' items={} subscribers=0",
                                            view_name, batch.inner.items.len()
                                        );
                                    }
                                }
                                if let Some(senders) = subscribers.get_mut(view_name) {
                                    senders.retain(|tx| {
                                        match tx.try_send(batch.clone()) {
                                            Ok(()) => true,
                                            Err(mpsc::error::TrySendError::Full(_)) => {
                                                // A dropped delta would silently corrupt the
                                                // subscriber's incremental state forever (lost
                                                // rows / ghost rows). Close the stream instead so
                                                // the consumer sees the end-of-stream and must
                                                // resubscribe via watch(), re-querying initial rows.
                                                tracing::error!(
                                                    "[MatviewManager] CDC subscriber for '{}' is full; \
                                                     closing its stream (delivering a partial delta \
                                                     stream would corrupt incremental consumers)",
                                                    view_name
                                                );
                                                false
                                            }
                                            Err(mpsc::error::TrySendError::Closed(_)) => false,
                                        }
                                    });
                                    if senders.is_empty() {
                                        subscribers.remove(view_name);
                                    }
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                // A broadcast lag means the demux missed `n` CDC
                                // batches — across ALL views, since the channel
                                // is shared. Every subscriber's incremental
                                // state is now potentially wrong (lost rows /
                                // ghost rows), exactly like the try_send Full
                                // case above but for every open stream. Warn-and-
                                // continue would silently corrupt them. Force-
                                // close every subscriber stream (drop the senders)
                                // so each consumer sees end-of-stream and must
                                // resubscribe via watch(), re-querying initial
                                // rows to recover a consistent baseline.
                                tracing::error!(
                                    "[MatviewManager] CDC demux lagged by {} messages; \
                                     closing all {} subscriber stream(s) (delivering a \
                                     partial delta stream would corrupt incremental \
                                     consumers)",
                                    n,
                                    subscribers.values().map(|s| s.len()).sum::<usize>()
                                );
                                subscribers.clear();
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                break;
                            }
                        }
                    }
                }
            }
        });

        cmd_tx
    }

    /// Drop all `watch_view_*` materialized views left over from a previous
    /// session.
    ///
    /// Turso IVM matviews can become stale across app restarts (e.g., when
    /// document UUIDs change or the underlying data is re-synced). Dropping
    /// them ensures they get recreated fresh with correct IVM state.
    pub async fn drop_stale_views(&self) -> Result<()> {
        let rows = self
            .db_handle
            .query(
                "SELECT name FROM sqlite_master WHERE type='view' AND name LIKE 'watch_view_%'",
                HashMap::new(),
            )
            .await?;

        for row in &rows {
            if let Some(Value::String(name)) = row.get("name") {
                tracing::info!("[MatviewManager] Dropping stale view: {}", name);
                let drop_sql = format!("DROP VIEW IF EXISTS {}", name);
                self.db_handle.execute_ddl(&drop_sql).await?;
                self.cleanup_orphaned_dbsp_tables(name).await?;
            }
        }

        // Reset the in-memory cache: every view tracked there is either one we
        // just dropped or one that was never registered to begin with.
        self.known_views.write().await.clear();

        if !rows.is_empty() {
            tracing::info!("[MatviewManager] Dropped {} stale watch views", rows.len());
        }

        Ok(())
    }

    /// Hash SQL text into a deterministic view name.
    pub fn compute_view_name(sql: &str) -> String {
        let mut hasher = DefaultHasher::new();
        sql.hash(&mut hasher);
        format!("watch_view_{:x}", hasher.finish())
    }

    /// Ensure a materialized view exists for the given SQL, creating it if
    /// needed.
    ///
    /// Steps: prime FDW cache (if applicable) → check existence → acquire DDL
    /// mutex → double-check → clean orphaned DBSP state tables → strip
    /// ORDER BY → CREATE MATERIALIZED VIEW with dependency tracking.
    #[tracing::instrument(skip(self, sql), fields(view_name = tracing::field::Empty))]
    pub async fn ensure_view(&self, sql: &str) -> Result<String> {
        self.prime_fdw_caches(sql).await?;

        let view_name = Self::compute_view_name(sql);
        tracing::Span::current().record("view_name", view_name.as_str());

        if self.is_view_known(&view_name).await {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(
                "[MatviewManager] View {} cached as known, reusing",
                view_name
            );
            return Ok(view_name);
        }

        if self.view_exists(&view_name).await {
            self.mark_view_known(&view_name).await;
            tracing::debug!(
                "[MatviewManager] View {} already exists, reusing",
                view_name
            );
            return Ok(view_name);
        }

        tracing::debug!(
            "[MatviewManager] View {} does not exist, creating...",
            view_name
        );

        let _ddl_guard = self.ddl_mutex.lock().await;
        tracing::debug!(
            "[MatviewManager] Acquired DDL mutex for view: {}",
            view_name
        );

        // Re-check the cache and sqlite_master under the DDL mutex — another
        // task may have created the view while we were waiting.
        if self.is_view_known(&view_name).await {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(
                "[MatviewManager] View {} cached while waiting for DDL mutex, reusing",
                view_name
            );
            return Ok(view_name);
        }
        if self.view_exists(&view_name).await {
            self.mark_view_known(&view_name).await;
            tracing::debug!(
                "[MatviewManager] View {} was created while waiting for DDL mutex, reusing",
                view_name
            );
            return Ok(view_name);
        }

        self.cleanup_orphaned_dbsp_tables(&view_name).await?;

        let sql_for_view = strip_order_by(sql);
        let create_view_sql = format!(
            "CREATE MATERIALIZED VIEW IF NOT EXISTS {} AS {}",
            view_name, sql_for_view
        );
        tracing::debug!(
            "[MatviewManager] Creating materialized view: {}",
            create_view_sql
        );

        let provides = vec![Resource::schema(view_name.clone())];
        // Fail loud: a parse failure here silently became "no dependencies",
        // which mis-orders matview creation and manifests as a boot HANG
        // ("waiting for dependencies") rather than an error. Surface it.
        let requires = parse_sql(&sql_for_view)
            .map(|stmts| extract_table_refs(&stmts))
            .with_context(|| {
                format!(
                    "MatviewManager::ensure_view: failed to parse SELECT SQL for matview \
                     '{view_name}' while extracting table dependencies; mis-ordered DDL would \
                     hang on missing deps. SQL: {sql_for_view}"
                )
            })?;

        tracing::debug!(
            "[MatviewManager] DDL deps — provides: {:?}, requires: {:?}",
            provides,
            requires
        );

        self.db_handle
            .execute_ddl_with_deps(&create_view_sql, provides, requires, priority::DDL_MATVIEW)
            .await
            // The inner error is spelled into the message, not only chained:
            // every consumer of this Err logs it with `{}`, so a `.context()`
            // source left the actual Turso/DDL-queue failure unrecorded and the
            // ledger blamed the statement instead of the cause.
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to create materialized view {view_name}: {create_view_sql} — cause: {e}"
                )
            })?;

        self.ddl_creates.fetch_add(1, Ordering::Relaxed);
        self.mark_view_known(&view_name).await;
        tracing::debug!("[MatviewManager] Successfully created view: {}", view_name);
        Ok(view_name)
    }

    /// Like `ensure_view` but retries on transient errors (for startup
    /// preloading).
    ///
    /// Logs warnings instead of failing — a preload failure is non-fatal
    /// because `watch_query` will create the view lazily later.
    pub async fn preload(&self, sql: &str) -> Result<String> {
        let view_name = Self::compute_view_name(sql);

        if self.is_view_known(&view_name).await {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(
                "[MatviewManager] preload: view {} cached as known, skipping",
                view_name
            );
            return Ok(view_name);
        }

        if self.view_exists(&view_name).await {
            self.mark_view_known(&view_name).await;
            tracing::debug!(
                "[MatviewManager] preload: view {} already exists, skipping",
                view_name
            );
            return Ok(view_name);
        }

        let sql_for_view = strip_order_by(sql);
        let create_view_sql = format!(
            "CREATE MATERIALIZED VIEW IF NOT EXISTS {} AS {}",
            view_name, sql_for_view
        );

        // A preload failure is disclosed, not fatal: `watch_query` creates the
        // view lazily later, so the app degrades to a cold first render rather
        // than failing to boot.
        match self.db_handle.execute_ddl(&create_view_sql).await {
            Ok(_) => {
                self.ddl_creates.fetch_add(1, Ordering::Relaxed);
                self.mark_view_known(&view_name).await;
                tracing::info!("[MatviewManager] preload: created view {}", view_name);
            }
            Err(e) => {
                tracing::warn!(
                    "[MatviewManager] preload: failed to create view {}: {}\n{}",
                    view_name,
                    e,
                    create_view_sql
                );
            }
        }
        Ok(view_name)
    }

    /// Query all rows from a materialized view.
    ///
    /// Includes Turso's internal `rowid` aliased as `_rowid` so that
    /// `LiveData` can build its `rowid → user-key` map for matview rows
    /// without an `id` column. Matches the shape `process_cdc_event`
    /// produces for live CDC events, where `_rowid` is injected into the
    /// `data` HashMap. Without this alignment, an initial row whose first
    /// post-load CDC event is a `Delete` (e.g. a `focus_roots` row whose
    /// region cursor is set to NULL via `NavigateBack` before any
    /// intermediate update) would never be removed from the LiveData.
    #[tracing::instrument(skip(self))]
    pub async fn query_view(&self, view_name: &str) -> Result<Vec<StorageEntity>> {
        self.query_view_ordered(view_name, None).await
    }

    /// `query_view` with the definition's `ORDER BY` re-applied.
    ///
    /// The matview body cannot carry an `ORDER BY` (Turso IVM rejects Sort),
    /// so `ensure_view` strips it. Re-applying it here is what keeps a watched
    /// query's snapshot in the same order a one-shot `execute_query` of the
    /// same SQL returns; without it the two disagree and the watched read comes
    /// back in rowid order.
    #[tracing::instrument(skip(self))]
    pub async fn query_view_ordered(
        &self,
        view_name: &str,
        order_by: Option<&str>,
    ) -> Result<Vec<StorageEntity>> {
        let select_sql = match order_by {
            Some(clause) => format!("SELECT *, rowid AS _rowid FROM {view_name} {clause}"),
            None => format!("SELECT *, rowid AS _rowid FROM {view_name}"),
        };
        self.db_handle
            .query(&select_sql, HashMap::new())
            .await
            .with_context(|| format!("Failed to query view {view_name}: {select_sql}"))
    }

    /// Output column names of a materialized view, so a source query's
    /// `ORDER BY` can be re-expressed against them.
    async fn view_columns(&self, view_name: &str) -> Result<Vec<String>> {
        let rows = self
            .db_handle
            .query(&format!("PRAGMA table_info({view_name})"), HashMap::new())
            .await
            .with_context(|| format!("PRAGMA table_info({view_name}) for the ORDER BY rewrite"))?;
        rows.iter()
            .map(|row| {
                row.get("name")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "PRAGMA table_info({view_name}) returned a row with no textual \
                             `name`: {row:?}"
                        )
                    })
            })
            .collect()
    }

    /// Subscribe to CDC for a specific view, returning a filtered stream.
    ///
    /// Registers with the single demultiplexer task instead of spawning a
    /// per-subscription filter task. The demux routes batches by
    /// `relation_name` and prunes closed subscribers automatically. Awaits
    /// the demux's registration ack before returning, so a subsequent query
    /// is guaranteed to observe registration-before-query ordering.
    pub async fn subscribe_cdc(&self, view_name: &str) -> Result<RowChangeStream> {
        let (tx, rx) = mpsc::channel(1024);
        let (ack_tx, ack_rx) = oneshot::channel();
        tracing::info!("[MatviewManager] subscribe_cdc('{}')", view_name);
        self.demux_cmd_tx
            .send(DemuxCommand::Subscribe {
                view_name: view_name.to_string(),
                tx,
                ack: ack_tx,
            })
            .await
            .map_err(|e| {
                anyhow::anyhow!("failed to register CDC subscriber for '{view_name}': {e}")
            })?;
        ack_rx
            .await
            .map_err(|_| anyhow::anyhow!("CDC demux dropped subscription ack for '{view_name}'"))?;
        Ok(tokio_stream::wrappers::ReceiverStream::new(rx))
    }

    /// Ensure a materialized view exists, query its initial data, and subscribe
    /// to CDC.
    #[tracing::instrument(skip(self, sql))]
    pub async fn watch(&self, sql: &str) -> Result<WatchResult> {
        let view_name = self.ensure_view(sql).await?;
        let stream = self.subscribe_cdc(&view_name).await?;
        // The clause comes off the SOURCE query, where its table aliases are in
        // scope; over the view they are not, so it has to be re-expressed in the
        // view's own output columns before it can be spliced onto the read.
        let order_by = match crate::util::trailing_order_by(sql) {
            Some(clause) => {
                let columns = self.view_columns(&view_name).await?;
                Some(
                    crate::util::rewrite_order_by_for_view(&clause, &columns).with_context(
                        || {
                            format!(
                                "watch: cannot re-apply `{clause}` over view {view_name}. Source \
                                 SQL: {sql}"
                            )
                        },
                    )?,
                )
            }
            None => None,
        };
        let initial_rows = self
            .query_view_ordered(&view_name, order_by.as_deref())
            .await?;
        Ok(WatchResult {
            initial_rows,
            stream,
            view_name,
        })
    }

    async fn view_exists(&self, view_name: &str) -> bool {
        self.exists_calls.fetch_add(1, Ordering::Relaxed);
        let check_sql = format!(
            "SELECT name FROM sqlite_master WHERE type='view' AND name='{}'",
            view_name
        );
        match self.db_handle.query(&check_sql, HashMap::new()).await {
            Ok(results) => !results.is_empty(),
            Err(_) => false,
        }
    }

    /// Whether the cache still vouches for `view_name`.
    ///
    /// A reap since the cache was established discards it wholesale rather than
    /// tracking which names died: reaps are rare next to hits, and each
    /// surviving view costs one `view_exists` probe to re-learn.
    async fn is_view_known(&self, view_name: &str) -> bool {
        let epoch = self.db_handle.reap_epoch();
        if self.validated_at.swap(epoch, Ordering::Relaxed) != epoch {
            self.known_views.write().await.clear();
            return false;
        }
        self.known_views.read().await.contains(view_name)
    }

    async fn mark_view_known(&self, view_name: &str) {
        self.known_views.write().await.insert(view_name.to_string());
    }

    /// Prime FDW-backed cache tables referenced in the SQL.
    ///
    /// For each table in the SQL that has an FDW counterpart (`{table}_fdw`),
    /// rewrite the SQL to query the FDW table. This triggers the FDW's
    /// write-through, populating the cache table. Then calls the hook.
    async fn prime_fdw_caches(&self, sql: &str) -> Result<()> {
        let fdw_tables = self.fdw_backed_tables.read().await;
        if fdw_tables.is_empty() {
            return Ok(());
        }

        // Fail loud: a parse failure here silently became "no table refs",
        // skipping FDW cache priming entirely and leaving the matview to read
        // stale/empty cache tables. Surface it instead.
        let table_refs = parse_sql(sql)
            .map(|stmts| extract_table_refs(&stmts))
            .with_context(|| {
                format!(
                    "MatviewManager::prime_fdw_caches: failed to parse SQL while extracting \
                     FDW-backed table references. SQL: {sql}"
                )
            })?;

        for resource in &table_refs {
            let table_name = resource.name();
            if fdw_tables.contains(table_name) {
                let fdw_sql = sql.replace(table_name, &format!("{table_name}_fdw"));
                tracing::info!(
                    "[MatviewManager] Priming FDW cache for '{}': {}",
                    table_name,
                    &fdw_sql[..fdw_sql.len().min(200)]
                );
                match self.db_handle.query(&fdw_sql, HashMap::new()).await {
                    Ok(rows) => {
                        tracing::info!(
                            "[MatviewManager] FDW prime: {} rows written through to '{}'",
                            rows.len(),
                            table_name,
                        );
                        // Notify hook (e.g. subscribe to resource notifications)
                        if let Some(hook) = self.hook.read().await.as_ref() {
                            hook.on_fdw_primed(table_name, &fdw_sql).await;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[MatviewManager] FDW prime failed for '{}': {e}",
                            table_name,
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// Disclose (never DROP) residual `__turso_internal_dbsp_state_*` tables.
    /// The `__turso_internal_` prefix is reserved: a user `DROP TABLE` bails
    /// `Cannot drop system table ...`. Disposal belongs to Turso's own
    /// CREATE/DROP MATERIALIZED VIEW path — see the free-function
    /// [`cleanup_orphaned_dbsp_state`] doc for the full rationale.
    async fn cleanup_orphaned_dbsp_tables(&self, view_name: &str) -> anyhow::Result<()> {
        cleanup_orphaned_dbsp_state(&self.db_handle, view_name).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ivm_maintainable_flags_every_subquery_predicate_spelling() {
        // Maintainable (no WHERE subquery predicate): plain filters, plain/
        // aliased joins, and the `LEFT JOIN … IS NULL` anti-join, which the fork
        // maintains CORRECTLY after the c6cfab7d populate fix (pinned end-to-end
        // by `left_join_isnull_matview_matches_fresh_after_populate_fix`).
        assert!(sql_ivm_maintainable(
            "SELECT id FROM block WHERE json_extract(properties,'$.task_state')='TODO'"
        ));
        assert!(sql_ivm_maintainable(
            "SELECT b.id FROM block b JOIN block_tags t ON t.block_id = b.id WHERE t.tag='agent'"
        ));
        assert!(sql_ivm_maintainable(
            "SELECT b.id FROM block b LEFT JOIN block_requires r ON r.block_id = b.id WHERE \
             r.block_id IS NULL"
        ));
        // A string literal containing "exists" must NOT be flagged — we visit the
        // typed AST nodes, so only a genuine `Exists`/`InSubquery` node counts,
        // never rendered text.
        assert!(sql_ivm_maintainable(
            "SELECT id FROM block WHERE content = 'this does not exists'"
        ));
        assert!(sql_ivm_maintainable(
            "SELECT id FROM block WHERE content = 'Exists {'"
        ));

        // Un-maintainable: EVERY subquery-predicate spelling, all silently-empty
        // over the chained `block` matview (verifier 2b). `NOT EXISTS`,
        // `NOT (EXISTS …)` (the keystone generator's spelling), plain `EXISTS`
        // (the fork refuses `Exists { negated: false }` — the operator itself),
        // `IN (subquery)` and `NOT IN (subquery)`, any nesting.
        for sql in [
            "SELECT b.* FROM block b WHERE NOT EXISTS (SELECT 1 FROM block_requires br WHERE \
             br.block_id=b.id)",
            "SELECT b.id FROM block b WHERE NOT (EXISTS (SELECT 1 FROM block_tags t WHERE \
             t.block_id=b.id))",
            "SELECT b.id FROM block b WHERE EXISTS (SELECT 1 FROM block_tags t WHERE \
             t.block_id=b.id)",
            "SELECT b.id FROM block b WHERE b.id IN (SELECT block_id FROM block_tags WHERE \
             tag='agent')",
            "SELECT b.id FROM block b WHERE b.id NOT IN (SELECT block_id FROM block_requires)",
            "SELECT 1 WHERE a OR NOT EXISTS (SELECT 1 FROM block_tags)",
            "select 1 from block where not   exists (select 1 from block_tags)",
        ] {
            assert!(
                !sql_ivm_maintainable(sql),
                "must route eager (un-maintainable subquery predicate): {sql}"
            );
        }

        // A query that does not parse is conservatively un-maintainable.
        assert!(!sql_ivm_maintainable("this is not valid sql !@#"));
    }

    #[test]
    fn normalize_collapses_whitespace_and_lowercases() {
        let stored = "CREATE MATERIALIZED VIEW current_focus AS\nSELECT\n    nc.region,\n    \
                      nh.block_id\nFROM navigation_cursor nc\nJOIN navigation_history nh ON \
                      nc.history_id = nh.id";
        let desired = "CREATE MATERIALIZED VIEW current_focus AS SELECT nc.region, nh.block_id \
                       FROM navigation_cursor nc JOIN navigation_history nh ON nc.history_id = \
                       nh.id";
        assert_eq!(normalize_view_sql(stored), normalize_view_sql(desired));
    }

    #[test]
    fn normalize_strips_trailing_semicolon() {
        assert_eq!(
            normalize_view_sql("SELECT 1;"),
            normalize_view_sql("SELECT 1")
        );
    }

    #[test]
    fn normalize_detects_actual_change() {
        let v1 = "CREATE MATERIALIZED VIEW foo AS SELECT id FROM block";
        let v2 = "CREATE MATERIALIZED VIEW foo AS SELECT id, content FROM block";
        assert_ne!(normalize_view_sql(v1), normalize_view_sql(v2));
    }

    // Word-boundary identifier matching drives the DDL dependency ordering
    // (a matview depending on another must be created after it). Pins the
    // positive match, the whitespace/underscore tokenizer boundary, and the
    // no-substring-false-positive contract.
    #[test]
    fn sql_references_identifier_matches_whole_tokens_only() {
        assert!(sql_references_identifier(
            "SELECT x FROM foo JOIN bar",
            "foo"
        ));
        // underscore is part of an identifier, not a delimiter (kills the
        // `== '_'` -> `!= '_'` split-predicate mutant).
        assert!(sql_references_identifier(
            "SELECT * FROM my_table",
            "my_table"
        ));
        // case-insensitive whole-token match.
        assert!(sql_references_identifier("select * from FOO", "foo"));
        // substring must NOT match (kills the `-> false`/`delete !` mutants that
        // would make every call match-nothing or match-everything).
        assert!(!sql_references_identifier("SELECT * FROM foobar", "foo"));
        assert!(!sql_references_identifier("SELECT * FROM other", "foo"));
    }

    #[test]
    fn compute_view_name_is_deterministic_prefixed_and_distinct() {
        let a = MatviewManager::compute_view_name("SELECT id FROM block");
        let a_again = MatviewManager::compute_view_name("SELECT id FROM block");
        let b = MatviewManager::compute_view_name("SELECT id, content FROM block");
        assert!(a.starts_with("watch_view_"), "got {a}");
        assert_eq!(a, a_again, "same SQL must yield same view name");
        assert_ne!(a, b, "different SQL must yield different view names");
    }

    /// Boot-time idempotency + crash recovery for `reconcile_named_view`.
    ///
    /// 1. First reconcile creates the matview.
    /// 2. A second reconcile with the SAME SELECT is a no-op (`Ok(false)`) —
    ///    the happy path every restart takes; it must never error with "already
    ///    exists" (the stale-DB boot-loop bug).
    /// 3. With an orphaned `__turso_internal_dbsp_state_*` table injected (the
    ///    residue a crash mid-DDL leaves behind) a definition change still
    ///    reconciles cleanly instead of boot-looping. The base table `src` —
    ///    the source of truth — is never dropped.
    #[tokio::test]
    async fn reconcile_named_view_is_idempotent_and_recovers_from_orphaned_state() {
        use crate::turso::TursoBackend;

        let (_backend, handle) = TursoBackend::new_in_memory()
            .await
            .expect("in-memory backend");
        handle
            .execute_ddl("CREATE TABLE IF NOT EXISTS src (id TEXT PRIMARY KEY, v TEXT)")
            .await
            .expect("create base table");
        handle
            .transition_to_ready()
            .await
            .expect("transition to ready");

        // 1. First boot creates the view.
        let created = reconcile_named_view(&handle, "src_view", "SELECT id, v FROM src")
            .await
            .expect("first reconcile");
        assert!(created, "first reconcile should create the view");

        // 2. Second boot with identical SELECT is a no-op — the regression guard
        //    against "table src_view already exists" boot-looping on restart.
        let created_again = reconcile_named_view(&handle, "src_view", "SELECT id, v FROM src")
            .await
            .expect("second reconcile must not error on an existing matview");
        assert!(!created_again, "identical reconcile should be a no-op");

        // 3. Simulate the crash residue: a leftover object occupying the matview's NAME
        //    with no committed `type='view'` row (what makes a plain `CREATE
        //    MATERIALIZED VIEW` die with "already exists" and boot-loop). A plain table
        //    standing on the name reproduces the collision; `reconcile_named_view` must
        //    recover, not fail.
        handle
            .execute_ddl("CREATE TABLE IF NOT EXISTS orphan_view (id TEXT)")
            .await
            .expect("inject orphaned backing table on the matview name");
        let recovered = reconcile_named_view(&handle, "orphan_view", "SELECT id FROM src")
            .await
            .expect("reconcile must recover from a name collision left by a crash");
        assert!(recovered, "collision recovery should (re)create the view");
        let as_view = handle
            .query(
                "SELECT name FROM sqlite_master WHERE type='view' AND name='orphan_view'",
                std::collections::HashMap::new(),
            )
            .await
            .expect("query view presence after recovery");
        assert_eq!(as_view.len(), 1, "orphan_view must now be a real matview");

        // Base table survived the whole dance — no user data loss.
        let rows = handle
            .query(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='src'",
                std::collections::HashMap::new(),
            )
            .await
            .expect("query base table presence");
        assert_eq!(
            rows.len(),
            1,
            "base table `src` must survive matview recovery"
        );

        handle.shutdown().await.expect("shutdown");
    }

    /// Regression: the boot-time DBSP-state cleanup must NEVER issue a
    /// `DROP TABLE __turso_internal_dbsp_state_*`. Turso reserves the
    /// `__turso_internal_` prefix and bails `Cannot drop system table ...`,
    /// which on Android surfaced as a stale-DB boot panic (older schema/DBSP
    /// epoch left an orphaned state table that the cleanup then tried to DROP).
    ///
    /// A live matview owns a real `__turso_internal_dbsp_state_v1_{view}`
    /// table; driving the cleanup while it exists reproduces the forbidden
    /// DROP without needing two DBSP circuit versions. Post-fix the cleanup
    /// discloses the residue but leaves disposal to Turso's own CREATE/DROP
    /// VIEW dbsp handling, so it must return `Ok` and the matview must survive.
    #[tokio::test]
    async fn cleanup_never_issues_forbidden_system_table_drop() {
        use crate::turso::TursoBackend;

        let (_backend, handle) = TursoBackend::new_in_memory()
            .await
            .expect("in-memory backend");
        handle
            .execute_ddl("CREATE TABLE IF NOT EXISTS src (id TEXT PRIMARY KEY, v TEXT)")
            .await
            .expect("create base table");
        handle
            .transition_to_ready()
            .await
            .expect("transition to ready");

        reconcile_named_view(&handle, "src_view", "SELECT id, v FROM src")
            .await
            .expect("create matview");

        // The matview owns a real system-prefixed DBSP state table.
        let dbsp = handle
            .query(
                "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE \
                 '__turso_internal_dbsp_state_v%_src_view'",
                HashMap::new(),
            )
            .await
            .expect("query dbsp state table");
        assert_eq!(dbsp.len(), 1, "matview must create a DBSP state table");

        // The exact panic path: cleanup must not attempt a forbidden DROP on
        // the system-prefixed DBSP state table.
        cleanup_orphaned_dbsp_state(&handle, "src_view")
            .await
            .expect("cleanup must not issue a forbidden system-table DROP");

        // Matview survived and still resolves against its base table.
        handle
            .execute_ddl("INSERT INTO src (id, v) VALUES ('x', '1')")
            .await
            .expect("insert into base");
        let rows = handle
            .query("SELECT id, v FROM src_view", HashMap::new())
            .await
            .expect("query matview after cleanup");
        assert_eq!(rows.len(), 1, "live matview must survive the cleanup");

        handle.shutdown().await.expect("shutdown");
    }

    /// Keystone rung for the persisted-DB boot ENVIRONMENT gap.
    ///
    /// The composed keystone PBT always starts fresh, so the entire
    /// "reconcile a derived matview over a persisted DB" class is invisible to
    /// it — the class the Android stale-DB boot panic belonged to. This rung
    /// boots a **file-backed** DB, creates a matview (persisting its backing
    /// btree AND its `__turso_internal_dbsp_state_v1_*` state), shuts down,
    /// reopens over the SAME file, and reconciles with a CHANGED definition —
    /// the DROP+CREATE-over-persisted-state path. It must complete without a
    /// `Cannot drop system table` panic, reflect the new definition, and leave
    /// the base table (source of truth) intact.
    #[tokio::test]
    async fn reconcile_over_persisted_db_survives_definition_change() {
        use crate::turso::TursoBackend;

        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("reboot.db");

        // ── Boot 1: create base table + matview, then persist ──────────────
        {
            let db = TursoBackend::open_database(&db_path).expect("open boot-1");
            let (cdc_tx, _rx) = tokio::sync::broadcast::channel(64);
            let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("backend boot-1");
            handle
                .execute_ddl("CREATE TABLE IF NOT EXISTS src (id TEXT PRIMARY KEY, v TEXT, w TEXT)")
                .await
                .expect("create base table");
            handle.transition_to_ready().await.expect("ready boot-1");
            handle
                .execute_ddl("INSERT INTO src (id, v, w) VALUES ('x', '1', 'keep')")
                .await
                .expect("seed base row");
            let created = reconcile_named_view(&handle, "src_view", "SELECT id, v FROM src")
                .await
                .expect("boot-1 create matview");
            assert!(created, "boot-1 should create the matview");
            handle.shutdown().await.expect("shutdown boot-1");
        }

        // ── Boot 2 over the SAME file: reconcile a CHANGED definition ──────
        let db = TursoBackend::open_database(&db_path).expect("open boot-2");
        let (cdc_tx, _rx) = tokio::sync::broadcast::channel(64);
        let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("backend boot-2");
        handle.transition_to_ready().await.expect("ready boot-2");

        // Adds a column to the SELECT — forces DROP+CREATE over the persisted
        // matview and its persisted DBSP state. Pre-fix this reconcile path
        // could hit `Cannot drop system table` on an orphaned state table.
        let changed = reconcile_named_view(&handle, "src_view", "SELECT id, v, w FROM src")
            .await
            .expect("boot-2 reconcile over persisted DB must not panic");
        assert!(changed, "changed definition should recreate the matview");

        let rows = handle
            .query("SELECT id, v, w FROM src_view", HashMap::new())
            .await
            .expect("query recreated matview");
        assert_eq!(rows.len(), 1, "recreated matview must project the base row");

        // Base table (source of truth) survived the reboot + reconcile.
        let base = handle
            .query("SELECT w FROM src WHERE id = 'x'", HashMap::new())
            .await
            .expect("query base table after reboot");
        assert_eq!(base.len(), 1, "base row must survive reboot + reconcile");

        handle.shutdown().await.expect("shutdown boot-2");
    }
}

/// Refuse to build `view_name` over any base table whose stored DDL declares
/// `ON CONFLICT REPLACE`.
///
/// The base set is taken by token-matching `select_sql` against the table names
/// in `sqlite_master`, which over-approximates toward REFUSING — the safe
/// direction for a guard whose job is keeping the pairing out of the tree.
async fn reject_on_conflict_replace_bases(
    db_handle: &DbHandle,
    view_name: &str,
    select_sql: &str,
) -> Result<()> {
    let rows = db_handle
        .query(
            "SELECT name, sql FROM sqlite_master WHERE type = 'table'",
            HashMap::new(),
        )
        .await?;
    let referenced = holon_turso_select_tokens(select_sql);
    for row in &rows {
        let (Some(Value::String(name)), Some(Value::String(ddl))) =
            (row.get("name"), row.get("sql"))
        else {
            continue;
        };
        if !referenced.iter().any(|t| t.eq_ignore_ascii_case(name)) {
            continue;
        }
        if crate::turso::declares_on_conflict_replace(ddl) {
            return Err(anyhow::anyhow!(
                "refusing to create materialized view `{view_name}` over base table `{name}`: \
                 that table declares ON CONFLICT REPLACE, so every plain INSERT into it carries \
                 REPLACE semantics — which on our Turso fork silently drops rows from every view \
                 over it (see crates/holon-turso/tests/replace_into_matview_base.rs; engine fix: \
                 fork bookmark `ivm-replace-double-old-row-capture`, PR #8463). Redeclare the \
                 table without the clause, or do not build a view on it."
            ));
        }
    }
    Ok(())
}

/// Identifier-ish tokens of a view's defining SELECT, for base-table matching.
fn holon_turso_select_tokens(select_sql: &str) -> Vec<String> {
    select_sql
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}
