use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU64;

use async_trait::async_trait;
use futures::future::FutureExt;
use serde_json;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_stream::wrappers::ReceiverStream;
use turso_core::Database;
use turso_core::DatabaseOpts;
use turso_core::MemoryIO;
use turso_core::OpenFlags;
#[cfg(target_family = "unix")]
use turso_core::UnixIO;
use turso_core::types::RelationChangeEvent;
use turso_sdk_kit::rsapi::DatabaseChangeType;
use turso_sdk_kit::rsapi::TursoConnection;
use turso_sdk_kit::rsapi::TursoDatabaseConfig;

/// Host-IO seam for wasm32: browser workers register their `turso_core::IO`
/// implementation (e.g. an OPFS shim) here before opening a file-backed
/// database. Insert-only — a second registration is a wiring bug.
#[cfg(all(not(target_family = "unix"), target_family = "wasm"))]
pub mod wasm_io {
    use std::cell::RefCell;
    use std::sync::Arc;

    // thread_local because host IO shims (e.g. the browser worker's OPFS
    // shim) are deliberately not Send/Sync; the engine and its Turso actor
    // all run on the worker's single napi thread.
    thread_local! {
        static IO: RefCell<Option<Arc<dyn turso_core::IO>>> = const { RefCell::new(None) };
    }

    pub fn register(io: Arc<dyn turso_core::IO>) {
        IO.with(|slot| {
            let mut slot = slot.borrow_mut();
            assert!(
                slot.is_none(),
                "wasm IO already registered — register must be called exactly once per thread"
            );
            *slot = Some(io);
        });
    }

    pub fn registered() -> Option<Arc<dyn turso_core::IO>> {
        IO.with(|slot| slot.borrow().clone())
    }
}

use holon_api::Batch;
use holon_api::BatchMetadata;
use holon_api::BatchTraceContext;
use holon_api::BatchWithMetadata;
use holon_api::CHANGE_ORIGIN_COLUMN;
use holon_api::Change;
use holon_api::ChangeOrigin;
use holon_api::Value;
use holon_core::storage::Filter;
use holon_core::storage::Resource;
use holon_core::storage::Result;
use holon_core::storage::StorageBackend;
use holon_core::storage::StorageEntity;
use holon_core::storage::StorageError;

use crate::matview_lease::LeaseGrant;
use crate::matview_lease::MatviewStats;
use crate::matview_lease::ViewState;
use crate::matview_lease::ViewWaiter;
use crate::sql_parser::extract_created_tables;
use crate::sql_parser::extract_table_refs;
use crate::sql_parser::parse_sql;
use crate::sql_utils::rewrite_named_params;

// ============================================================================
// Types moved from turso_actor.rs
// ============================================================================

/// Database operation phase for observability and debugging
///
/// Note: DDL is allowed in ALL phases because MatViews are created dynamically
/// when users navigate to blocks with PRQL queries. The actor's value is
/// SERIALIZATION, not phase-based blocking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DatabasePhase {
    /// Startup phase - schema initialization in progress
    #[default]
    SchemaInit,
    /// Normal operation - all DDL complete, application running
    Ready,
    /// Shutting down - rejecting new commands
    ShuttingDown,
}

/// Priority levels for different operation types.
pub mod priority {
    /// Core schema DDL (blocks, commands, etc.)
    pub const DDL_CORE: u32 = 100;
    /// Module-specific DDL (todoist tables, etc.)
    pub const DDL_MODULE: u32 = 90;
    /// Materialized views
    pub const DDL_MATVIEW: u32 = 50;
    /// Data manipulation operations
    pub const DML: u32 = 0;
}

/// Unique identifier for a pending DDL operation.
pub type OperationId = u64;

/// A pending DDL operation with dependency information.
struct PendingDdl {
    id: OperationId,
    sql: String,
    provides: Vec<Resource>,
    requires: Vec<Resource>,
    priority: u32,
    completion: DdlCompletion,
}

/// Who is waiting on a DDL operation's result.
enum DdlCompletion {
    /// The caller that submitted the DDL.
    Caller(oneshot::Sender<Result<()>>),
    /// A `CREATE MATERIALIZED VIEW` issued to satisfy a view lease: on success
    /// the view flips to `Live` and every parked waiter is granted.
    ViewCreate { view_name: String },
}

/// Commands that can be sent to the database actor
pub enum DbCommand {
    /// Execute a query (SELECT) with named parameters and return results
    Query {
        sql: String,
        params: HashMap<String, Value>,
        response: oneshot::Sender<Result<Vec<StorageEntity>>>,
    },

    /// Execute a query (SELECT) with positional parameters and return results
    QueryPositional {
        sql: String,
        params: Vec<turso::Value>,
        response: oneshot::Sender<Result<Vec<StorageEntity>>>,
    },

    /// Execute a statement (INSERT, UPDATE, DELETE) and return affected row
    /// count
    Execute {
        sql: String,
        params: Vec<turso::Value>,
        response: oneshot::Sender<Result<u64>>,
    },

    /// Execute DDL (CREATE TABLE, CREATE VIEW, etc.) immediately
    ExecuteDdl {
        sql: String,
        response: oneshot::Sender<Result<()>>,
    },

    /// Execute DDL with explicit dependency tracking
    ExecuteDdlWithDeps {
        sql: String,
        provides: Vec<Resource>,
        requires: Vec<Resource>,
        priority: u32,
        response: oneshot::Sender<Result<()>>,
    },

    /// Execute DDL with auto-inferred dependencies
    ExecuteDdlAuto {
        sql: String,
        priority: u32,
        response: oneshot::Sender<Result<()>>,
    },

    /// Mark resources as available (for bootstrapping existing schemas)
    MarkAvailable { resources: Vec<Resource> },

    /// Check if a resource is currently available
    ResourceExists {
        resource: Resource,
        response: oneshot::Sender<bool>,
    },

    /// Execute multiple statements in a transaction
    Transaction {
        statements: Vec<(String, Vec<turso::Value>)>,
        response: oneshot::Sender<Result<()>>,
    },

    /// Subscribe to CDC events for a specific relation
    SubscribeCdc {
        relation: String,
        response: oneshot::Sender<Result<broadcast::Receiver<BatchWithMetadata<RowChange>>>>,
    },

    /// Transition to Ready phase (called after all startup DDL is complete)
    TransitionToReady {
        response: oneshot::Sender<Result<()>>,
    },

    /// Get current database phase
    GetPhase {
        response: oneshot::Sender<DatabasePhase>,
    },

    /// Register a foreign data wrapper as a virtual table
    RegisterForeignTable {
        name: String,
        fdw: std::sync::Arc<dyn turso_core::foreign::ForeignDataWrapper>,
        response: oneshot::Sender<Result<()>>,
    },

    /// Take a lease on a `watch_view_*` materialized view, creating it if the
    /// actor does not already own it. `requires` is parsed by the caller so a
    /// malformed SELECT fails there instead of stalling the DDL queue.
    AcquireViewLease {
        view_name: String,
        select_sql: String,
        requires: Vec<Resource>,
        response: oneshot::Sender<Result<LeaseGrant>>,
    },

    /// Give back a lease. One-way: the reap it may trigger runs inline in this
    /// command, so there is nothing for the releaser to wait on.
    ReleaseViewLease {
        view_name: String,
        lease_id: u64,
        generation: u64,
    },

    /// Create a view if absent and hold it open for the life of the process,
    /// through any number of later lease cycles.
    EnsurePinnedView {
        view_name: String,
        select_sql: String,
        requires: Vec<Resource>,
        response: oneshot::Sender<Result<()>>,
    },

    /// Drop every `watch_view_%` in the database, forget all view state, and
    /// start a new lease generation. Answers with the number of views dropped.
    ResetWatchViews {
        response: oneshot::Sender<Result<usize>>,
    },

    /// Graceful shutdown
    Shutdown { response: oneshot::Sender<()> },
}

/// State the actor owns for the whole of its life, mutated only while a
/// command is being processed.
struct ActorState {
    phase: DatabasePhase,
    pending_ddl: VecDeque<PendingDdl>,
    available_resources: HashSet<Resource>,
    next_op_id: OperationId,
    /// Lifetime of every `watch_view_*` the actor owns.
    views: HashMap<String, ViewState>,
    /// Bumped by `ResetWatchViews`; grants from earlier generations are inert.
    generation: u64,
    next_lease_id: u64,
    matview_stats: Arc<MatviewStats>,
}

impl ActorState {
    fn new(matview_stats: Arc<MatviewStats>) -> Self {
        Self {
            phase: DatabasePhase::SchemaInit,
            pending_ddl: VecDeque::new(),
            available_resources: HashSet::new(),
            next_op_id: 1,
            views: HashMap::new(),
            generation: 0,
            next_lease_id: 1,
            matview_stats,
        }
    }

    fn next_op_id(&mut self) -> OperationId {
        let id = self.next_op_id;
        self.next_op_id += 1;
        id
    }

    fn next_lease_id(&mut self) -> u64 {
        let id = self.next_lease_id;
        self.next_lease_id += 1;
        id
    }

    fn publish_matview_stats(&self) {
        self.matview_stats.publish(&self.views);
    }

    /// True while `view_name` is owned AND still has a reason to stay alive.
    fn is_held(&self, view_name: &str) -> bool {
        match self.views.get(view_name) {
            Some(ViewState::Creating { .. }) => true,
            Some(ViewState::Live { leases, pinned }) => *leases > 0 || *pinned,
            None => false,
        }
    }
}

/// Stable short fingerprint of named bound parameters, recorded on `query`
/// spans as `params_fp`. The PBT N+1 reporter groups duplicate SQL texts by
/// this, so a parameterized statement fired for N *different* bindings
/// (fan-out — possibly a real N+1, judge by count) is distinguishable from
/// the same statement + bindings executed twice (definitely redundant work).
/// Values are hashed, never logged, so row content stays out of traces.
fn named_params_fingerprint(params: &HashMap<String, Value>) -> String {
    use std::hash::Hash;
    use std::hash::Hasher;
    if params.is_empty() {
        return "-".to_string();
    }
    let mut entries: Vec<(&str, String)> = params
        .iter()
        .map(|(k, v)| (k.as_str(), format!("{v:?}")))
        .collect();
    entries.sort();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    entries.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// The `sql` span attribute: the identity every SQL-metrics consumer buckets
/// by. Head AND tail, because a head-only prefix cannot separate Holon's block
/// queries — the full-table hydrating scan, the doc-scoped CTE and the
/// single-block point read share ~900 characters of column list and differ only
/// in the trailing FROM/WHERE, so prefix-bucketing merged three consumers into
/// one and misattributed which of them was re-querying.
///
/// Head+tail alone still MERGES statements that differ only in the elided
/// middle (the recursive-descendants family: bare parent-chain vs full
/// descendants row). Merging always LOOSENS the dedup gate — two statements in
/// one bucket over-subtract their combined excess — so a truncated fingerprint
/// carries a hash of the FULL text, making it injective in the SQL while the
/// readable head/tail survives for the reader.
fn sql_fingerprint(sql: &str) -> String {
    use std::hash::Hash;
    use std::hash::Hasher;

    const HEAD: usize = 200;
    // The block queries' last edge subquery alone is ~150 characters, so a
    // shorter tail stops before the FROM/WHERE that tells them apart.
    const TAIL: usize = 240;
    let chars: Vec<char> = sql.chars().collect();
    if chars.len() <= HEAD + TAIL {
        return sql.to_string();
    }
    let head: String = chars[..HEAD].iter().collect();
    let tail: String = chars[chars.len() - TAIL..].iter().collect();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    sql.hash(&mut hasher);
    format!("{head} …#{:016x}… {tail}", hasher.finish())
}

/// Positional-parameter sibling of [`named_params_fingerprint`], for
/// `execute` spans.
fn positional_params_fingerprint(params: &[turso::Value]) -> String {
    use std::hash::Hash;
    use std::hash::Hasher;
    if params.is_empty() {
        return "-".to_string();
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for v in params {
        format!("{v:?}").hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

/// Handle for sending commands to the database actor
///
/// This is the public API for database operations. Clone freely - all clones
/// share the same underlying actor and CDC broadcast channel.
#[derive(Clone)]
/// @c4 code
pub struct DbHandle {
    tx: mpsc::Sender<DbCommand>,
    cdc_broadcast: broadcast::Sender<BatchWithMetadata<RowChange>>,
    /// Monotonic counter assigned to each non-empty CDC batch immediately
    /// before broadcast. Cloned `DbHandle`s share the same `Arc<AtomicU64>`,
    /// so any handle reads the same global emission watermark.
    cdc_seq: Arc<std::sync::atomic::AtomicU64>,
    /// Matview lease counters, republished by the actor after every lease
    /// mutation so a reader never has to queue behind the command stream.
    matview_stats: Arc<MatviewStats>,
}

impl DbHandle {
    /// Execute a query (SELECT) with named parameters and return results
    // 120 chars is NOT enough to tell Holon's block queries apart: the
    // full-table hydrating scan, the doc-scoped CTE and the single-block point
    // read share a longer prefix than that, so a shorter fingerprint merges
    // three different consumers into one bucket and misattributes redundancy.
    #[tracing::instrument(skip(self, params), fields(sql = %sql_fingerprint(sql), params_fp = %named_params_fingerprint(&params)))]
    pub async fn query(
        &self,
        sql: &str,
        params: HashMap<String, Value>,
    ) -> Result<Vec<StorageEntity>> {
        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .send(DbCommand::Query {
                sql: sql.to_string(),
                params,
                response: response_tx,
            })
            .await
            .map_err(|_| StorageError::DatabaseError("Actor channel closed".to_string()))?;

        response_rx
            .await
            .map_err(|_| StorageError::DatabaseError("Actor response channel closed".to_string()))?
    }

    /// Execute a query (SELECT) with positional parameters and return results
    pub async fn query_positional(
        &self,
        sql: &str,
        params: Vec<turso::Value>,
    ) -> Result<Vec<StorageEntity>> {
        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .send(DbCommand::QueryPositional {
                sql: sql.to_string(),
                params,
                response: response_tx,
            })
            .await
            .map_err(|_| StorageError::DatabaseError("Actor channel closed".to_string()))?;

        response_rx
            .await
            .map_err(|_| StorageError::DatabaseError("Actor response channel closed".to_string()))?
    }

    /// Execute a statement (INSERT, UPDATE, DELETE) with positional
    /// `holon_api::Value` parameters. Storage-agnostic callers (e.g.
    /// holon-orgmode) use this so they never name `turso::Value` directly;
    /// same actor path (and CDC emission) as [`execute`](Self::execute).
    pub async fn execute_values(&self, sql: &str, params: Vec<Value>) -> Result<u64> {
        let params = params.iter().map(value_to_turso_param).collect();
        self.execute(sql, params).await
    }

    /// Execute a statement (INSERT, UPDATE, DELETE) and return affected row
    /// count
    #[tracing::instrument(skip(self, params), fields(sql = %sql_fingerprint(sql), params_fp = %positional_params_fingerprint(&params)))]
    pub async fn execute(&self, sql: &str, params: Vec<turso::Value>) -> Result<u64> {
        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .send(DbCommand::Execute {
                sql: sql.to_string(),
                params,
                response: response_tx,
            })
            .await
            .map_err(|_| StorageError::DatabaseError("Actor channel closed".to_string()))?;

        response_rx
            .await
            .map_err(|_| StorageError::DatabaseError("Actor response channel closed".to_string()))?
    }

    /// Execute DDL (CREATE TABLE, CREATE VIEW, etc.)
    #[tracing::instrument(skip(self), fields(sql = %sql_fingerprint(sql)))]
    pub async fn execute_ddl(&self, sql: &str) -> Result<()> {
        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .send(DbCommand::ExecuteDdl {
                sql: sql.to_string(),
                response: response_tx,
            })
            .await
            .map_err(|_| StorageError::DatabaseError("Actor channel closed".to_string()))?;

        let outcome = response_rx.await.map_err(|_| {
            StorageError::DatabaseError("Actor response channel closed".to_string())
        })?;
        // Every drop issued from OUTSIDE the actor arrives here — the base-view
        // rebuild cascade (`drop_dependent_views`), advice synthesis, the MCP
        // sidecar. Bumping at this boundary rather than at each of them is what
        // stops a future caller from dropping a view that readers still believe
        // in. The actor's own reaps never pass through here and bump themselves.
        if outcome.is_ok() && drops_a_view(sql) {
            self.matview_stats.note_reap();
        }
        outcome
    }

    /// Register a foreign data wrapper as a virtual table.
    ///
    /// The table becomes immediately queryable via SQL.
    pub async fn register_foreign_table(
        &self,
        name: &str,
        fdw: std::sync::Arc<dyn turso_core::foreign::ForeignDataWrapper>,
    ) -> Result<()> {
        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .send(DbCommand::RegisterForeignTable {
                name: name.to_string(),
                fdw,
                response: response_tx,
            })
            .await
            .map_err(|_| StorageError::DatabaseError("Actor channel closed".to_string()))?;

        response_rx
            .await
            .map_err(|_| StorageError::DatabaseError("Actor response channel closed".to_string()))?
    }

    /// Execute multiple statements in a transaction
    pub async fn transaction(&self, statements: Vec<(String, Vec<turso::Value>)>) -> Result<()> {
        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .send(DbCommand::Transaction {
                statements,
                response: response_tx,
            })
            .await
            .map_err(|_| StorageError::DatabaseError("Actor channel closed".to_string()))?;

        response_rx
            .await
            .map_err(|_| StorageError::DatabaseError("Actor response channel closed".to_string()))?
    }

    /// Subscribe to CDC events for a specific relation
    pub async fn subscribe_cdc(
        &self,
        relation: &str,
    ) -> Result<broadcast::Receiver<BatchWithMetadata<RowChange>>> {
        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .send(DbCommand::SubscribeCdc {
                relation: relation.to_string(),
                response: response_tx,
            })
            .await
            .map_err(|_| StorageError::DatabaseError("Actor channel closed".to_string()))?;

        response_rx
            .await
            .map_err(|_| StorageError::DatabaseError("Actor response channel closed".to_string()))?
    }

    /// Transition to Ready phase
    ///
    /// Call this after all startup DDL is complete. This signals to the system
    /// that the database schema is stable and background tasks can begin.
    pub async fn transition_to_ready(&self) -> Result<()> {
        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .send(DbCommand::TransitionToReady {
                response: response_tx,
            })
            .await
            .map_err(|_| StorageError::DatabaseError("Actor channel closed".to_string()))?;

        response_rx
            .await
            .map_err(|_| StorageError::DatabaseError("Actor response channel closed".to_string()))?
    }

    /// Get current database phase
    pub async fn get_phase(&self) -> Result<DatabasePhase> {
        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .send(DbCommand::GetPhase {
                response: response_tx,
            })
            .await
            .map_err(|_| StorageError::DatabaseError("Actor channel closed".to_string()))?;

        response_rx
            .await
            .map_err(|_| StorageError::DatabaseError("Actor response channel closed".to_string()))
    }

    /// Graceful shutdown
    pub async fn shutdown(&self) -> Result<()> {
        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .send(DbCommand::Shutdown {
                response: response_tx,
            })
            .await
            .map_err(|_| StorageError::DatabaseError("Actor channel closed".to_string()))?;

        response_rx.await.map_err(|_| {
            StorageError::DatabaseError("Actor response channel closed".to_string())
        })?;
        Ok(())
    }

    /// Execute DDL with explicit dependency tracking.
    ///
    /// The actor ensures dependencies are satisfied before execution.
    /// Operations are queued until their required resources are available.
    ///
    /// # Arguments
    /// * `sql` - The DDL SQL to execute
    /// * `provides` - Resources this operation creates
    /// * `requires` - Resources this operation depends on
    /// * `priority` - Execution priority (higher = sooner among ready
    ///   operations)
    #[tracing::instrument(skip(self, provides, requires), fields(sql = %sql_fingerprint(sql)))]
    pub async fn execute_ddl_with_deps(
        &self,
        sql: &str,
        provides: Vec<Resource>,
        requires: Vec<Resource>,
        priority: u32,
    ) -> Result<()> {
        use std::time::Duration;

        let requires_for_error = requires.clone();
        let sql_preview: String = sql.chars().take(80).collect();

        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .send(DbCommand::ExecuteDdlWithDeps {
                sql: sql.to_string(),
                provides,
                requires,
                priority,
                response: response_tx,
            })
            .await
            .map_err(|_| StorageError::DatabaseError("Actor channel closed".to_string()))?;

        // Timeout to detect missing mark_available() calls.
        // wasm32 has no tokio runtime under wasm_bindgen_futures::spawn_local,
        // so tokio::time::timeout would panic — await directly there.
        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        {
            const DEPENDENCY_TIMEOUT: Duration = Duration::from_secs(120);
            match tokio::time::timeout(DEPENDENCY_TIMEOUT, response_rx).await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err(StorageError::DatabaseError(
                    "Actor response channel closed".to_string(),
                )),
                Err(_elapsed) => {
                    let missing_resources: Vec<String> = requires_for_error
                        .iter()
                        .map(|r| r.name().to_string())
                        .collect();

                    Err(StorageError::DatabaseError(format!(
                        "DDL timed out after {:?} waiting for dependencies.\nSQL: \
                         {}...\nRequired: {:?}\n\nCall mark_available() for resources created \
                         outside the actor.",
                        DEPENDENCY_TIMEOUT, sql_preview, missing_resources
                    )))
                }
            }
        }
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        {
            let _ = (requires_for_error, sql_preview);
            match response_rx.await {
                Ok(result) => result,
                Err(_) => Err(StorageError::DatabaseError(
                    "Actor response channel closed".to_string(),
                )),
            }
        }
    }

    /// Execute DDL with auto-inferred dependencies.
    ///
    /// Dependencies are extracted from the SQL using sqlparser.
    pub async fn execute_ddl_auto(&self, sql: &str, priority: u32) -> Result<()> {
        use std::time::Duration;

        let sql_preview: String = sql.chars().take(80).collect();

        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .send(DbCommand::ExecuteDdlAuto {
                sql: sql.to_string(),
                priority,
                response: response_tx,
            })
            .await
            .map_err(|_| StorageError::DatabaseError("Actor channel closed".to_string()))?;

        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        {
            const DEPENDENCY_TIMEOUT: Duration = Duration::from_secs(120);
            match tokio::time::timeout(DEPENDENCY_TIMEOUT, response_rx).await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err(StorageError::DatabaseError(
                    "Actor response channel closed".to_string(),
                )),
                Err(_elapsed) => {
                    let inferred_deps = parse_sql(sql)
                        .map(|stmts| extract_table_refs(&stmts))
                        .unwrap_or_default();
                    let missing_resources: Vec<String> =
                        inferred_deps.iter().map(|r| r.name().to_string()).collect();

                    Err(StorageError::DatabaseError(format!(
                        "DDL timed out after {:?} waiting for dependencies.\nSQL: {}...\nInferred \
                         required: {:?}\n\nCall mark_available() for resources created outside \
                         the actor.",
                        DEPENDENCY_TIMEOUT, sql_preview, missing_resources
                    )))
                }
            }
        }
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        {
            let _ = sql_preview;
            match response_rx.await {
                Ok(result) => result,
                Err(_) => Err(StorageError::DatabaseError(
                    "Actor response channel closed".to_string(),
                )),
            }
        }
    }

    /// Mark resources as available (for bootstrapping existing schemas).
    ///
    /// Call this during startup to register tables that already exist.
    pub async fn mark_available(&self, resources: Vec<Resource>) -> Result<()> {
        self.tx
            .send(DbCommand::MarkAvailable { resources })
            .await
            .map_err(|_| StorageError::DatabaseError("Actor channel closed".to_string()))
    }

    /// Check if a resource is currently available.
    ///
    /// Returns true if the resource has been marked as available (either by DDL
    /// execution or by explicit `mark_available()` call).
    pub async fn resource_exists(&self, resource: &Resource) -> Result<bool> {
        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .send(DbCommand::ResourceExists {
                resource: resource.clone(),
                response: response_tx,
            })
            .await
            .map_err(|_| StorageError::DatabaseError("Actor channel closed".to_string()))?;

        response_rx
            .await
            .map_err(|_| StorageError::DatabaseError("Actor response channel closed".to_string()))
    }

    // --- Matview leases ---

    /// Take a lease on the `watch_view_*` matview built from `select_sql`,
    /// creating it if the actor does not already own it. The view lives until
    /// the returned grant (and every other outstanding one) is released.
    pub async fn acquire_view_lease(
        &self,
        view_name: &str,
        select_sql: &str,
    ) -> Result<LeaseGrant> {
        let requires = Self::view_dependencies(view_name, select_sql)?;
        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .send(DbCommand::AcquireViewLease {
                view_name: view_name.to_string(),
                select_sql: select_sql.to_string(),
                requires,
                response: response_tx,
            })
            .await
            .map_err(|_| StorageError::DatabaseError("Actor channel closed".to_string()))?;
        Self::await_view_response(response_rx, "acquire_view_lease", view_name).await
    }

    /// Create the view if absent and hold it open for the life of the process.
    /// A pin is never released, so later lease cycles cannot reap the view.
    pub async fn ensure_pinned_view(&self, view_name: &str, select_sql: &str) -> Result<()> {
        let requires = Self::view_dependencies(view_name, select_sql)?;
        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .send(DbCommand::EnsurePinnedView {
                view_name: view_name.to_string(),
                select_sql: select_sql.to_string(),
                requires,
                response: response_tx,
            })
            .await
            .map_err(|_| StorageError::DatabaseError("Actor channel closed".to_string()))?;
        Self::await_view_response(response_rx, "ensure_pinned_view", view_name).await
    }

    /// Give back a lease. Fire-and-forget by design: the reap it may trigger
    /// happens inside the actor, so a releasing `Drop` never blocks and never
    /// needs a runtime.
    pub fn release_view_lease(&self, view_name: &str, grant: LeaseGrant) {
        let cmd = DbCommand::ReleaseViewLease {
            view_name: view_name.to_string(),
            lease_id: grant.lease_id,
            generation: grant.generation,
        };
        match self.tx.try_send(cmd) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(cmd)) => {
                // Dropping the release would pin the view forever. Hand it to a
                // task that can wait for queue space instead.
                tracing::warn!(
                    view = %view_name,
                    "[DbHandle] actor queue full on matview lease release; deferring the release \
                     to a spawned sender"
                );
                let tx = self.tx.clone();
                crate::util::spawn_actor(async move {
                    let _ = tx.send(cmd).await;
                });
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::debug!(
                    view = %view_name,
                    "[DbHandle] actor gone before matview lease release; the database it owned is \
                     gone with it"
                );
            }
        }
    }

    /// Drop every `watch_view_%` in the database and start a new lease
    /// generation. Returns how many views were dropped.
    pub async fn reset_watch_views(&self) -> Result<usize> {
        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .send(DbCommand::ResetWatchViews {
                response: response_tx,
            })
            .await
            .map_err(|_| StorageError::DatabaseError("Actor channel closed".to_string()))?;
        response_rx
            .await
            .map_err(|_| StorageError::DatabaseError("Actor response channel closed".to_string()))?
    }

    /// Current matview lease counters.
    pub fn matview_stats(&self) -> crate::matview_lease::MatviewStatsSnapshot {
        self.matview_stats.snapshot()
    }

    /// Table dependencies of a view's SELECT, parsed caller-side.
    ///
    /// Fail loud: a swallowed parse error becomes "no dependencies", which
    /// mis-orders the CREATE and shows up as a boot hang ("waiting for
    /// dependencies") rather than an error.
    fn view_dependencies(view_name: &str, select_sql: &str) -> Result<Vec<Resource>> {
        parse_sql(select_sql)
            .map(|stmts| extract_table_refs(&stmts))
            .map_err(|e| {
                StorageError::DatabaseError(format!(
                    "matview '{view_name}': failed to parse its SELECT while extracting table \
                     dependencies; a mis-ordered CREATE would hang waiting for dependencies \
                     instead of failing. SQL: {select_sql}. Parse error: {e}"
                ))
            })
    }

    /// Await an actor reply that may be parked behind a `CREATE MATERIALIZED
    /// VIEW` waiting for its base tables — same bound as `execute_ddl_*`.
    async fn await_view_response<T>(
        response_rx: oneshot::Receiver<Result<T>>,
        op: &str,
        view_name: &str,
    ) -> Result<T> {
        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        {
            const DEPENDENCY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
            match tokio::time::timeout(DEPENDENCY_TIMEOUT, response_rx).await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err(StorageError::DatabaseError(format!(
                    "{op}('{view_name}'): actor response channel closed"
                ))),
                Err(_elapsed) => Err(StorageError::DatabaseError(format!(
                    "{op}('{view_name}') timed out after {DEPENDENCY_TIMEOUT:?} — its CREATE is \
                     still waiting for base tables. Call mark_available() for resources created \
                     outside the actor."
                ))),
            }
        }
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        {
            match response_rx.await {
                Ok(result) => result,
                Err(_) => Err(StorageError::DatabaseError(format!(
                    "{op}('{view_name}'): actor response channel closed"
                ))),
            }
        }
    }

    /// Get a reference to the CDC broadcast sender.
    pub fn cdc_broadcast(&self) -> &broadcast::Sender<BatchWithMetadata<RowChange>> {
        &self.cdc_broadcast
    }

    /// Current reap epoch of this database — see `MatviewStats::reap_epoch`.
    pub(crate) fn reap_epoch(&self) -> u64 {
        self.matview_stats.reap_epoch()
    }

    /// A witness for "which database is this?", shared by every clone of this
    /// handle and distinct between databases. `Weak` so a registry keyed by it
    /// can tell a live database from a dead one whose allocation was reused.
    pub(crate) fn database_witness(&self) -> std::sync::Weak<std::sync::atomic::AtomicU64> {
        Arc::downgrade(&self.cdc_seq)
    }

    /// Subscribe to the CDC broadcast channel for raw row-level change events.
    pub fn subscribe_row_changes(&self) -> broadcast::Receiver<BatchWithMetadata<RowChange>> {
        self.cdc_broadcast.subscribe()
    }

    /// Highest CDC batch sequence number broadcast since process start.
    ///
    /// Tests and drivers can sample this immediately after a write completes
    /// (Turso's IVM is synchronous in the commit path, so any matview deltas
    /// have already been pushed onto the broadcast channel by the time
    /// `execute(..).await` returns) and then wait until every relevant
    /// subscriber's consumed seq is at least this high — replacing the
    /// fixed `tokio::time::sleep(50ms)` "let CDC settle" pattern.
    ///
    /// `0` means "no batch has been emitted yet".
    pub fn cdc_emitted_watermark(&self) -> u64 {
        self.cdc_seq.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Subscribe to CDC events as a stream.
    ///
    /// Converts the broadcast receiver into an mpsc-based `ReceiverStream`
    /// so callers get a `Stream` interface with backpressure.
    pub fn row_changes(&self) -> RowChangeStream {
        let mut broadcast_rx = self.cdc_broadcast.subscribe();
        let (tx, rx) = mpsc::channel(1024);
        crate::util::spawn_actor(async move {
            loop {
                match broadcast_rx.recv().await {
                    Ok(batch) => {
                        if tx.send(batch).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("[DbHandle] CDC subscriber lagged by {} messages", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        ReceiverStream::new(rx)
    }
}

// ============================================================================
// Helper functions moved from turso_actor.rs
// ============================================================================

/// Extract ChangeOrigin from row data's _change_origin column
fn extract_change_origin_from_data(data: &StorageEntity) -> ChangeOrigin {
    data.get(CHANGE_ORIGIN_COLUMN)
        .and_then(|v| match v {
            Value::String(json) => ChangeOrigin::from_json(json),
            _ => None,
        })
        .unwrap_or(ChangeOrigin::Remote {
            operation_id: None,
            trace_id: None,
        })
}

/// Convert holon_api::Value to turso::Value for parameter binding
pub(crate) fn value_to_turso_param(value: &Value) -> turso::Value {
    match value {
        Value::String(s) => turso::Value::Text(s.clone()),
        Value::Integer(i) => turso::Value::Integer(*i),
        Value::Float(f) => turso::Value::Real(*f),
        Value::Boolean(b) => turso::Value::Integer(if *b { 1 } else { 0 }),
        Value::DateTime(s) => turso::Value::Text(s.clone()),
        Value::Json(s) => turso::Value::Text(s.clone()),
        Value::Array(arr) => {
            let json_arr: Vec<serde_json::Value> = arr
                .iter()
                .map(|v| serde_json::Value::from(v.clone()))
                .collect();
            turso::Value::Text(serde_json::to_string(&serde_json::Value::Array(json_arr)).unwrap())
        }
        Value::Object(obj) => {
            let json_obj: serde_json::Map<String, serde_json::Value> = obj
                .iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::from(v.clone())))
                .collect();
            turso::Value::Text(serde_json::to_string(&serde_json::Value::Object(json_obj)).unwrap())
        }
        Value::Null => turso::Value::Null,
    }
}

/// Bind named parameters (`$name`, `:name`, `@name`) to positional
/// placeholders (`?`).
///
/// A placeholder with no bound value is an error, never a pass-through: SQLite
/// happily accepts the unbound name and evaluates it as NULL, so the statement
/// succeeds and quietly matches nothing.
fn bind_parameters(
    sql: &str,
    params: &HashMap<String, Value>,
) -> Result<(String, Vec<turso::Value>)> {
    let mut param_values = Vec::new();
    let mut unbound: Vec<String> = Vec::new();

    let result_sql = rewrite_named_params(sql, &mut |name| match params.get(name) {
        Some(value) => {
            param_values.push(value_to_turso_param(value));
            Some("?".to_string())
        }
        None => {
            unbound.push(name.to_string());
            None
        }
    });

    if !unbound.is_empty() {
        return Err(StorageError::QueryError(format!(
            "Parameters {unbound:?} appear in the query but have no bound value; an unbound \
             placeholder evaluates to NULL and would silently match nothing. Query: {sql}"
        )));
    }

    Ok((result_sql, param_values))
}

/// Convert turso_core::Value to holon_api::Value.
///
/// TEXT comes back verbatim as `Value::String` — JSON parsing is driven by
/// KNOWN JSON columns (see [`normalize_known_json_columns`]), never by
/// content sniffing. Sniffing reshaped user text like `"[1, 2, 3]"` into
/// `Value::Array` on the query path while the CDC path kept it a String,
/// so consumers diffing initial rows against CDC updates saw a spurious
/// type change.
fn turso_value_to_value(value: turso_core::Value) -> Value {
    match value {
        turso_core::Value::Null => Value::Null,
        turso_core::Value::Numeric(turso_core::Numeric::Integer(i)) => Value::Integer(i),
        turso_core::Value::Numeric(turso_core::Numeric::Float(f)) => Value::Float(f.into()),
        turso_core::Value::Text(s) => Value::String(s.to_string()),
        turso_core::Value::Blob(_) => Value::Null,
    }
}

/// Parse a Value that may be JSON object text or already an Object into a
/// HashMap.
fn parse_json_object(value: Value) -> Option<HashMap<String, Value>> {
    match value {
        Value::Object(obj) => Some(obj),
        Value::String(s) => serde_json::from_str::<serde_json::Value>(&s)
            .ok() // ALLOW(ok): non-JSON values become Null
            .and_then(|v| {
                if let serde_json::Value::Object(map) = v {
                    Some(map.into_iter().map(|(k, v)| (k, Value::from(v))).collect())
                } else {
                    None
                }
            }),
        _ => None,
    }
}

/// Normalize the KNOWN JSON columns of a row, applied identically on the
/// query path (`handle_query`/`handle_query_positional`) and the CDC path
/// (`parse_row_values_with_schema`) so both paths produce the same
/// representation for the same row:
///
/// - `data`: synthesized by the UNION-query rewriter (`json_object(*) AS data`,
///   see sql_parser.rs) — parsed and flattened into top-level fields.
/// - `properties`: the JSON object column on `block_raw` — parsed into
///   `Value::Object` (Null / non-object becomes an empty Object).
///
/// Everything else stays exactly as stored (parse-don't-validate: the JSON
/// column set comes from what our schema/rewriter declare, not from content
/// shape). In particular the `json_group_array` projection columns
/// (`tags`/`requires`) remain JSON TEXT; their consumers (`Block::try_from`
/// via `require_string_array`, the PBT row parsers) strictly parse that JSON
/// at their own boundary and already accepted TEXT because the CDC path
/// never sniffed.
fn normalize_known_json_columns(entity: &mut StorageEntity) {
    if let Some(data_value) = entity.remove("data")
        && let Some(obj) = parse_json_object(data_value)
    {
        for (key, value) in obj {
            entity.entry(key.into()).or_insert(value);
        }
    }

    if let Some(props) = entity.remove("properties") {
        entity.insert(
            "properties".into(),
            match parse_json_object(props) {
                Some(obj) => Value::Object(obj),
                None => Value::Object(HashMap::new()),
            },
        );
    }
}

// ============================================================================
// Original turso.rs types
// ============================================================================

pub(crate) fn default_turso_config() -> TursoDatabaseConfig {
    TursoDatabaseConfig {
        path: String::new(),
        experimental_features: None,
        // TODO(async_io): switch to `true` to match the public Builder
        // default and stop blocking tokio worker threads inside SQL IO.
        // The pre-fix matview-cursor-first-open bug used to fire on the
        // IO yield boundary, which made `false` defensible; nightscape@holon
        // 290fbb4ff fixed that, so the rationale is gone. Needs a workspace
        // test+PBT pass and live MCP/TUI/GPUI smoke before flipping — see
        // resolution devlog 2026-05-08-mcp-first-query-empty-matview.
        async_io: false,
        encryption: None,
        vfs: turso_sdk_kit::IoBackend::Default,
        io: None,
        db_file: None,
    }
}

/// A change notification from a materialized view
///
/// Note: The row_changes() method automatically coalesces DELETE+INSERT pairs
/// into UPDATE events to prevent UI flicker when materialized views are
/// updated.
///
/// **IMPORTANT - UI Keying Requirements**:
///
/// The `id` field in `ChangeData` is the SQLite ROWID, which is:
/// - Unique per view (not globally unique)
/// - Can be reused after DELETE operations
/// - Used for transport and coalescing only
///
/// **UI MUST KEY BY ENTITY ID from `data.get("id")`, NOT BY ROWID**
///
/// Example (illustrative — `change` is a `RowChange` you received):
/// ```rust,ignore
/// match change.change {
///     ChangeData::Created { data, .. } => {
///         let entity_id = data.get("id").unwrap(); // Use this for widget key
///         // Don't use ROWID (from `data.get("_rowid")`) as widget key!
///     }
///     ChangeData::Updated { id: rowid, data, .. } => {
///         let entity_id = data.get("id").unwrap(); // Use this for widget key
///         // Don't use `rowid` as widget key!
///     }
///     ChangeData::Deleted { id: entity_id, .. } => {
///         // Use entity_id directly - it's extracted from the deleted row data
///     }
/// }
/// ```
#[derive(Debug, Clone)]
pub struct RowChange {
    pub relation_name: String,
    pub change: ChangeData,
}

/// The type of change and associated data
///
/// **Note**: For `Created` and `Updated` variants, the ROWID is stored in
/// `data["_rowid"]`. For `Deleted`, the `id` field is the entity ID (extracted
/// from the deleted row data). See `RowChange` documentation for UI keying
/// requirements.
pub type ChangeData = Change<StorageEntity>;

/// Strip the Turso-side `relation_name` wrapper: consumers downstream of the
/// demux (e.g. `LiveData`) operate on the neutral `Change<StorageEntity>`.
impl From<RowChange> for ChangeData {
    fn from(rc: RowChange) -> Self {
        rc.change
    }
}

/// Stream of batched view changes with metadata
pub type RowChangeStream = ReceiverStream<BatchWithMetadata<RowChange>>;

/// Coalesce CDC row changes within a batch to prevent UI flicker.
///
/// - DELETE + INSERT for the same (relation, entity_id) → UPDATE
/// - INSERT + DELETE for the same (relation, entity_id) → no-op (both dropped)
/// - All other changes pass through unchanged
///
/// This is a pure function suitable for both synchronous use in
/// `process_cdc_event()` and as the `merge` function for
/// `holon_api::reactive::coalesce()`.
pub(crate) fn coalesce_row_changes(changes: Vec<RowChange>) -> Vec<RowChange> {
    let mut slots: Vec<Option<RowChange>> = changes.into_iter().map(Some).collect();
    let mut pending_deletes: HashMap<(String, String), usize> = HashMap::new();
    let mut pending_inserts: HashMap<(String, String), usize> = HashMap::new();

    for idx in 0..slots.len() {
        let Some(change) = slots[idx].clone() else {
            continue;
        };

        let entity_id = match &change.change {
            ChangeData::Deleted { id, .. } => id.clone(),
            ChangeData::Created { data, .. } => data
                .get("id")
                .and_then(|v| match v {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                })
                .or_else(|| {
                    data.get("_rowid").and_then(|v| match v {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    })
                })
                .unwrap_or_default(),
            ChangeData::Updated { id, .. } => id.clone(),
            ChangeData::FieldsChanged { entity_id, .. } => entity_id.clone(),
        };
        let key = (change.relation_name.clone(), entity_id);

        match &change.change {
            ChangeData::Deleted { .. } => {
                if let Some(insert_idx) = pending_inserts.remove(&key) {
                    // INSERT then DELETE → no-op
                    slots[insert_idx] = None;
                    slots[idx] = None;
                } else {
                    pending_deletes.insert(key, idx);
                }
            }
            ChangeData::Created { data, origin } => {
                let rowid = data
                    .get("_rowid")
                    .and_then(|v| match v {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();

                if let Some(delete_idx) = pending_deletes.remove(&key) {
                    // DELETE then INSERT → UPDATE (use entity ID, not ROWID)
                    slots[delete_idx] = None;
                    let entity_id = data
                        .get("id")
                        .and_then(|v| match v {
                            Value::String(s) => Some(s.clone()),
                            _ => None,
                        })
                        .unwrap_or(rowid);
                    slots[idx] = Some(RowChange {
                        relation_name: change.relation_name.clone(),
                        change: ChangeData::Updated {
                            id: entity_id,
                            data: data.clone(),
                            origin: origin.clone(),
                        },
                    });
                } else {
                    pending_inserts.insert(key, idx);
                }
            }
            ChangeData::Updated { .. } | ChangeData::FieldsChanged { .. } => {}
        }
    }

    slots.into_iter().flatten().collect()
}

// Removed in favour of `coalesce_row_changes` alone, after the Turso pin
// 290fbb4ff fixed the recursive-CTE matview UPDATE → INSERT-then-DELETE
// surface (it now arrives as a single `Update` or DELETE-then-INSERT pair,
// both of which `coalesce_row_changes` already handles). The
// `collapse_insert_delete_pairs` + `data_equal_ignoring_metadata` helpers
// and their unit tests went with it. Verified by re-running
// `turso_ivm_split_block_cdc_drop_repro` — all 6 characterisation tests
// pass without the pre-pass.

// ============================================================================
// SQL tracing
// ============================================================================

fn full_sql_tracing() -> bool {
    static FULL: OnceLock<bool> = OnceLock::new();
    *FULL.get_or_init(|| std::env::var("HOLON_TRACE_SQL").is_ok())
}

/// The object name a DDL statement targets (the matview/table/index name),
/// for the `holon_latency` `matview_ddl` stage. Best-effort token scan: the
/// name follows the `VIEW`/`TABLE`/`INDEX` keyword, past any `IF NOT EXISTS`.
fn ddl_target_name(sql: &str) -> &str {
    let mut tokens = sql.split_whitespace();
    while let Some(tok) = tokens.next() {
        if tok.eq_ignore_ascii_case("view")
            || tok.eq_ignore_ascii_case("table")
            || tok.eq_ignore_ascii_case("index")
        {
            for name in tokens.by_ref() {
                if name.eq_ignore_ascii_case("if")
                    || name.eq_ignore_ascii_case("not")
                    || name.eq_ignore_ascii_case("exists")
                {
                    continue;
                }
                return name.trim_matches(|c: char| c == '"' || c == '`' || c == '(');
            }
        }
    }
    "ddl"
}

/// Whether `sql` removes a view — `DROP VIEW`, with or without `IF EXISTS`.
/// Turso spells a materialized view's removal the same way.
fn drops_a_view(sql: &str) -> bool {
    let mut tokens = sql.split_whitespace();
    let Some(first) = tokens.next() else {
        return false;
    };
    first.eq_ignore_ascii_case("drop")
        && tokens
            .next()
            .is_some_and(|kind| kind.eq_ignore_ascii_case("view"))
}

fn trace_sql(tag: &str, sql: &str) {
    if full_sql_tracing() {
        // In release builds, workspace-hack's `release_max_level_info` compiles
        // `trace!`/`debug!` out, so we mirror to stderr in a format
        // `turso-sql-replay` can parse. Debug builds get the trace! macro live
        // and the eprintln is pure overhead (each call formats a fresh
        // timestamp into an stderr that is typically discarded by the parent
        // process — measured to ~2x startup time on org-pkm initial scan).
        #[cfg(not(debug_assertions))]
        {
            let ts = chrono::DateTime::from_timestamp_millis(holon_api::clock::now_millis())
                .expect("now within range")
                .format("%Y-%m-%dT%H:%M:%S%.6f");
            eprintln!("{ts}Z TRACE holon::storage::turso: [TursoBackend] {tag}: {sql}");
        }
        tracing::trace!("[TursoBackend] {tag}: {sql}");
    } else {
        tracing::trace!("[TursoBackend] {tag}: {}", &sql[..sql.len().min(120)]);
    }
}

fn trace_sql_positional(tag: &str, sql: &str, params: &[turso::Value]) {
    if full_sql_tracing() && !params.is_empty() {
        #[cfg(not(debug_assertions))]
        {
            let ts = chrono::DateTime::from_timestamp_millis(holon_api::clock::now_millis())
                .expect("now within range")
                .format("%Y-%m-%dT%H:%M:%S%.6f");
            eprintln!(
                "{ts}Z TRACE holon::storage::turso: [TursoBackend] {tag}: {sql} -- params: \
                 {params:?}"
            );
        }
        tracing::trace!("[TursoBackend] {tag}: {sql} -- params: {params:?}");
    } else {
        trace_sql(tag, sql);
    }
}

/// Named-parameter variant. Emits a stable `key=Value(...)` form so
/// `inline_named_params` in turso-sql-replay can substitute back.
fn trace_sql_named(tag: &str, sql: &str, params: &HashMap<String, Value>) {
    if !full_sql_tracing() {
        trace_sql(tag, sql);
        return;
    }
    if params.is_empty() {
        trace_sql(tag, sql);
        return;
    }
    let mut parts: Vec<String> = params.iter().map(|(k, v)| format!("{k}={v:?}")).collect();
    parts.sort();
    let params_str = parts.join(", ");
    #[cfg(not(debug_assertions))]
    {
        let ts = chrono::DateTime::from_timestamp_millis(holon_api::clock::now_millis())
            .expect("now within range")
            .format("%Y-%m-%dT%H:%M:%S%.6f");
        eprintln!(
            "{ts}Z TRACE holon::storage::turso: [TursoBackend] {tag}: {sql} -- params: \
             {params_str}"
        );
    }
    tracing::trace!("[TursoBackend] {tag}: {sql} -- params: {params_str}");
}

// ============================================================================
// TursoBackend with merged actor logic
// ============================================================================

/// @c4 code
/// @c4 uses DbHandle "actor handle" "call"
pub struct TursoBackend {
    db: Arc<Database>,
    /// Broadcast channel for CDC events - all subscribers share this channel.
    cdc_broadcast: broadcast::Sender<BatchWithMetadata<RowChange>>,
    /// Command channel sender for creating DbHandles
    tx: mpsc::Sender<DbCommand>,
    /// Monotonic per-process counter assigned to each CDC batch as it is
    /// broadcast. Cloned `DbHandle`s share this `Arc<AtomicU64>` so any
    /// reader observes the same emission watermark.
    cdc_seq: Arc<std::sync::atomic::AtomicU64>,
    /// Matview lease counters, written by the actor and read via `DbHandle`.
    matview_stats: Arc<MatviewStats>,
}

impl std::fmt::Debug for TursoBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TursoBackend")
            .field("db", &"Arc<Database>")
            .field(
                "cdc_broadcast",
                &format!(
                    "broadcast::Sender(receivers={})",
                    self.cdc_broadcast.receiver_count()
                ),
            )
            .field("tx", &"mpsc::Sender<DbCommand>")
            .finish()
    }
}

/// Turso-based storage backend
/// Note that this is the Turso Database, not Turso libsql.
///
/// From the docs:
/// How is Turso Database different from Turso's libSQL?
/// Turso Database is a project to build the next evolution of SQLite in Rust,
/// with a strong open contribution focus and features like native async
/// support, vector search, and more. The libSQL project is also an attempt to
/// evolve SQLite in a similar direction, but through a fork rather than a
/// rewrite. Rewriting SQLite in Rust started as an unassuming experiment, and
/// due to its incredible success, replaces libSQL as our intended direction.
impl TursoBackend {
    /// Open a Turso database file and return the Database handle.
    ///
    /// This is used internally by `new()` to create the database before setting
    /// up the actor.
    ///
    /// # Platform Support
    /// - **Unix-like systems** (macOS, Linux, BSD, iOS): Full file-based
    ///   storage support via UnixIO
    /// - **Windows**: Not yet supported
    #[cfg(target_family = "unix")]
    pub fn open_database<P: AsRef<Path>>(db_path: P) -> Result<Arc<Database>> {
        let db_path_str = db_path
            .as_ref()
            .to_str()
            .ok_or_else(|| StorageError::DatabaseError("Invalid path".to_string()))?;

        // `with_index_method(true)` unlocks the experimental `CREATE INDEX ..
        // USING <method>` surface (the Tantivy-backed `fts` method and the
        // sparse-vector method). Native-only: this block is `cfg(unix)`; the
        // wasm `open_database` below leaves it off (fts is cfg'd out of
        // turso_core on wasm anyway).
        let opts = DatabaseOpts::default()
            .with_views(true)
            .with_index_method(true);

        let db = if db_path_str.starts_with(":memory:") {
            let io = Arc::new(MemoryIO::new());
            Database::open_file_with_flags(
                io,
                db_path_str,
                OpenFlags::default(),
                opts,
                None,
                Arc::new(turso_core::SqliteDialect),
            )
        } else {
            let io =
                Arc::new(UnixIO::new().map_err(|e| StorageError::DatabaseError(e.to_string()))?);
            Database::open_file_with_flags(
                io,
                db_path_str,
                OpenFlags::default(),
                opts,
                None,
                Arc::new(turso_core::SqliteDialect),
            )
        }
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        tracing::info!("Turso database opened at: {}", db_path_str);
        Ok(db)
    }

    #[cfg(all(not(target_family = "unix"), target_family = "wasm"))]
    pub fn open_database<P: AsRef<Path>>(db_path: P) -> Result<Arc<Database>> {
        // wasm32: `:memory:` uses MemoryIO; any other path requires a host IO
        // registered via `register_wasm_io` (the browser worker registers its
        // OPFS shim before engine init). Fail loud if a file path is requested
        // without one — silently falling back to memory would fake persistence.
        let db_path_str = db_path
            .as_ref()
            .to_str()
            .ok_or_else(|| StorageError::DatabaseError("Invalid path".to_string()))?;
        let opts = DatabaseOpts::default().with_views(true);
        let db = if db_path_str.starts_with(":memory:") {
            let io = Arc::new(MemoryIO::new());
            Database::open_file_with_flags(
                io,
                db_path_str,
                OpenFlags::default(),
                opts,
                None,
                Arc::new(turso_core::SqliteDialect),
            )
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?
        } else {
            let io = wasm_io::registered().ok_or_else(|| {
                StorageError::DatabaseError(format!(
                    "open_database('{db_path_str}'): no wasm IO registered — call \
                     holon_turso::register_wasm_io (e.g. with the OPFS shim) before opening a \
                     file-backed database on wasm32"
                ))
            })?;
            Database::open_file_with_flags(
                io,
                db_path_str,
                OpenFlags::Create,
                opts,
                None,
                Arc::new(turso_core::SqliteDialect),
            )
            .map_err(|e| StorageError::DatabaseError(e.to_string()))?
        };
        tracing::info!("Turso database opened (wasm32) at: {}", db_path_str);
        Ok(db)
    }

    #[cfg(all(not(target_family = "unix"), not(target_family = "wasm")))]
    pub fn open_database<P: AsRef<Path>>(_: P) -> Result<Arc<Database>> {
        Err(StorageError::DatabaseError(
            "File-based storage not yet supported on this platform".to_string(),
        ))
    }

    /// Create a new TursoBackend, spawning an internal actor for database
    /// operations.
    ///
    /// This creates a single connection that is owned by the actor and
    /// processes all commands sequentially, eliminating race conditions.
    ///
    /// Returns `(Self, DbHandle)` - the backend and a handle for sending
    /// commands.
    pub fn new(
        db: Arc<Database>,
        cdc_broadcast: broadcast::Sender<BatchWithMetadata<RowChange>>,
    ) -> Result<(Self, DbHandle)> {
        use std::sync::atomic::AtomicU64;
        use std::sync::atomic::Ordering;
        // Create connection for actor
        let conn = Self::create_connection_internal(&db)?;

        // Process-monotonic CDC sequence shared with every cloned `DbHandle`.
        // Stamped onto the batch metadata before broadcast so subscribers can
        // implement "wait until consumed_seq >= cdc_emitted_watermark()".
        let cdc_seq = Arc::new(AtomicU64::new(0));

        // Set up CDC callback to broadcast to all subscribers
        if full_sql_tracing() {
            let ts = chrono::DateTime::from_timestamp_millis(holon_api::clock::now_millis())
                .expect("now within range")
                .format("%Y-%m-%dT%H:%M:%S%.6f");
            eprintln!(
                "{ts}Z TRACE holon::storage::turso: [TursoBackend] set_change_callback: \
                 registering CDC callback"
            );
        }
        tracing::trace!("[TursoBackend] set_change_callback: registering CDC callback");
        let cdc_tx_for_callback = cdc_broadcast.clone();
        let cdc_seq_for_callback = cdc_seq.clone();
        let actor_stats: Option<Arc<crate::turso_actor_stats::ActorStats>> =
            crate::turso_actor_stats::enabled_interval()
                .map(|_| crate::turso_actor_stats::ActorStats::new());
        let cdc_stats_for_callback = actor_stats.clone();
        conn.set_change_callback(move |event: &RelationChangeEvent| {
            tracing::trace!(
                "[TursoBackend CDC] relation='{}' raw_changes={}",
                event.relation_name,
                event.changes.len()
            );
            let raw_count = event.changes.len() as u64;
            let mut batch = Self::process_cdc_event(event);
            tracing::trace!(
                "[TursoBackend CDC] relation='{}' after_coalesce={}",
                event.relation_name,
                batch.inner.items.len()
            );
            if let Some(stats) = &cdc_stats_for_callback {
                stats.record_cdc(
                    &event.relation_name,
                    raw_count,
                    batch.inner.items.len() as u64,
                );
            }
            if !batch.inner.items.is_empty() {
                let next = cdc_seq_for_callback.fetch_add(1, Ordering::SeqCst) + 1;
                batch.metadata.seq = next;
                let _ = cdc_tx_for_callback.send(batch);
            }
        })
        .map_err(|e| StorageError::DatabaseError(format!("Failed to set CDC callback: {}", e)))?;

        // Create command channel
        let (tx, rx) = mpsc::channel(256);

        // Spawn actor loop. On wasm32 tokio's single-threaded runtime is
        // not actually polled (Dioxus-web drives futures via
        // wasm_bindgen_futures), so we route the actor through
        // wasm_bindgen_futures::spawn_local instead.
        let cdc_broadcast_for_actor = cdc_broadcast.clone();
        let actor_stats_for_actor = actor_stats.clone();
        let matview_stats = Arc::new(MatviewStats::default());
        let matview_stats_for_actor = matview_stats.clone();
        if let (Some(stats), Some(interval)) = (
            actor_stats.clone(),
            crate::turso_actor_stats::enabled_interval(),
        ) {
            tracing::info!(
                "[TursoBackend] HOLON_ACTOR_STATS enabled, logging every {:?}",
                interval
            );
            crate::turso_actor_stats::spawn_logger(stats, interval);
        }
        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        tokio::spawn(Self::run_actor(
            rx,
            conn,
            cdc_broadcast_for_actor,
            actor_stats_for_actor,
            matview_stats_for_actor,
        ));
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        wasm_bindgen_futures::spawn_local(Self::run_actor(
            rx,
            conn,
            cdc_broadcast_for_actor,
            actor_stats_for_actor,
            matview_stats_for_actor,
        ));

        tracing::info!(
            "[TursoBackend] Created - all database operations will be serialized through internal \
             actor"
        );

        let backend = Self {
            db,
            cdc_broadcast: cdc_broadcast.clone(),
            tx: tx.clone(),
            cdc_seq: cdc_seq.clone(),
            matview_stats: matview_stats.clone(),
        };
        let handle = DbHandle {
            tx,
            cdc_broadcast,
            cdc_seq,
            matview_stats,
        };

        Ok((backend, handle))
    }

    /// Create a new TursoBackend with an in-memory database.
    ///
    /// Used by tests and by the wasm32 browser demo.
    pub async fn new_in_memory() -> Result<(Self, DbHandle)> {
        let db = Self::open_database(":memory:")?;
        let (cdc_tx, _cdc_rx) = broadcast::channel(1024);
        Self::new(db, cdc_tx)
    }

    /// Get a handle to send commands to the database actor.
    pub fn handle(&self) -> DbHandle {
        DbHandle {
            tx: self.tx.clone(),
            cdc_broadcast: self.cdc_broadcast.clone(),
            cdc_seq: self.cdc_seq.clone(),
            matview_stats: self.matview_stats.clone(),
        }
    }

    /// Get a reference to the CDC broadcast channel.
    pub fn cdc_broadcast(&self) -> &broadcast::Sender<BatchWithMetadata<RowChange>> {
        &self.cdc_broadcast
    }

    /// Create a connection from database (internal helper).
    fn create_connection_internal(db: &Arc<Database>) -> Result<turso::Connection> {
        use std::sync::atomic::Ordering;
        static CONNECTION_COUNTER: AtomicU64 = AtomicU64::new(0);
        let conn_id = CONNECTION_COUNTER.fetch_add(1, Ordering::SeqCst);

        tracing::debug!("[CONN-{}] Creating new raw database connection...", conn_id);

        let conn_core = db.connect().map_err(|e| {
            tracing::error!("[CONN-{}] Failed to create connection: {}", conn_id, e);
            StorageError::DatabaseError(e.to_string())
        })?;

        // Enforce foreign keys DB-wide: FK checking is per-connection in the
        // fork, so every connection minted here opts in. This is the single
        // place that makes the block_raw parent FK (roots → sentinel:no_parent)
        // actually enforced on writes.
        conn_core.set_foreign_keys_enabled(true);

        let turso_conn = TursoConnection::new(&default_turso_config(), conn_core);
        let conn = turso::Connection::create(turso_conn, None);

        // Set busy timeout to prevent indefinite hangs on lock contention
        const BUSY_TIMEOUT_SECS: u64 = 30;
        if let Err(e) = conn.busy_timeout(std::time::Duration::from_secs(BUSY_TIMEOUT_SECS)) {
            tracing::warn!(
                "[CONN-{}] Failed to set busy_timeout on raw connection: {}",
                conn_id,
                e
            );
        }

        let autocommit = conn.is_autocommit().unwrap_or(true);
        tracing::debug!(
            "[CONN-{}] Raw connection created with busy_timeout={}s. Autocommit: {}",
            conn_id,
            BUSY_TIMEOUT_SECS,
            autocommit
        );

        Ok(conn)
    }

    /// Get a new connection to the database for direct SQL access.
    ///
    /// This creates a fresh connection without CDC callbacks. Use this for:
    /// - Test code that needs direct SQL access
    /// - Read-only queries that don't need CDC
    /// - Debugging and inspection
    ///
    /// For writes that should trigger CDC, use `handle()` methods instead.
    pub fn get_connection(&self) -> Result<turso::Connection> {
        Self::create_connection_internal(&self.db)
    }

    /// Helper to parse a row of turso_core::Value into our Entity type using
    /// schema
    pub fn parse_row_values_with_schema(
        values: &[turso_core::Value],
        columns: &[Arc<str>],
    ) -> StorageEntity {
        let mut entity = StorageEntity::with_capacity(values.len());

        for (idx, value) in values.iter().enumerate() {
            let our_value = match value {
                turso_core::Value::Null => Value::Null,
                turso_core::Value::Numeric(turso_core::Numeric::Integer(i)) => Value::Integer(*i),
                turso_core::Value::Numeric(turso_core::Numeric::Float(f)) => {
                    Value::Float((*f).into())
                }
                turso_core::Value::Text(s) => Value::String(s.to_string()),
                turso_core::Value::Blob(_) => Value::Null,
            };

            // Clone the shared column-name Arc; only the out-of-schema case allocates
            let column_name = columns.get(idx).map(Arc::clone).unwrap_or_else(|| {
                tracing::debug!(
                    "Warning: Column index {} exceeds schema length {}",
                    idx,
                    columns.len()
                );
                Arc::from("unknown")
            });

            entity.insert(column_name, our_value);
        }

        normalize_known_json_columns(&mut entity);

        entity
    }

    pub fn value_to_sql_param(&self, value: &Value) -> String {
        super::sql_utils::value_to_sql_literal(value)
    }

    fn build_where_clause(&self, filter: &Filter, params: &mut Vec<turso::Value>) -> String {
        match filter {
            Filter::Eq(field, value) => {
                params.push(value_to_turso_param(value));
                format!("{} = ?", field)
            }
            Filter::In(field, values) => {
                let placeholders = values
                    .iter()
                    .map(|v| {
                        params.push(value_to_turso_param(v));
                        "?"
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{} IN ({})", field, placeholders)
            }
            Filter::And(filters) => {
                let clauses = filters
                    .iter()
                    .map(|f| self.build_where_clause(f, params))
                    .collect::<Vec<_>>()
                    .join(" AND ");
                format!("({})", clauses)
            }
            Filter::Or(filters) => {
                let clauses = filters
                    .iter()
                    .map(|f| self.build_where_clause(f, params))
                    .collect::<Vec<_>>()
                    .join(" OR ");
                format!("({})", clauses)
            }
            Filter::IsNull(field) => format!("{} IS NULL", field),
            Filter::IsNotNull(field) => format!("{} IS NOT NULL", field),
        }
    }

    // ========================================================================
    // Actor loop and internal handlers
    // ========================================================================

    /// Process a CDC event into a BatchWithMetadata<RowChange>
    fn process_cdc_event(event: &RelationChangeEvent) -> BatchWithMetadata<RowChange> {
        let mut raw_changes = Vec::new();
        let mut batch_trace_context: Option<BatchTraceContext> = None;

        // Convert column names to Arc<str> once per event; rows below only Arc::clone
        let columns: Vec<Arc<str>> = event
            .columns
            .iter()
            .map(|c| Arc::from(c.as_str()))
            .collect();

        for change in event.changes.iter() {
            let change_data = match &change.change {
                DatabaseChangeType::Insert { .. } => {
                    if let Some(values) = change.parse_record() {
                        let mut data =
                            TursoBackend::parse_row_values_with_schema(&values, &columns);
                        data.insert("_rowid".into(), Value::String(change.id.to_string()));
                        let origin = extract_change_origin_from_data(&data);
                        if batch_trace_context.is_none() {
                            batch_trace_context = origin.to_batch_trace_context();
                        }
                        ChangeData::Created { data, origin }
                    } else {
                        continue;
                    }
                }
                DatabaseChangeType::Update { .. } => {
                    if let Some(values) = change.parse_record() {
                        let mut data =
                            TursoBackend::parse_row_values_with_schema(&values, &columns);
                        data.insert("_rowid".into(), Value::String(change.id.to_string()));
                        let origin = extract_change_origin_from_data(&data);
                        if batch_trace_context.is_none() {
                            batch_trace_context = origin.to_batch_trace_context();
                        }
                        let entity_id = data
                            .get("id")
                            .and_then(|v| match v {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            })
                            .unwrap_or_else(|| change.id.to_string());
                        ChangeData::Updated {
                            id: entity_id,
                            data,
                            origin,
                        }
                    } else {
                        continue;
                    }
                }
                DatabaseChangeType::Delete { .. } => {
                    if let Some(values) = change.parse_record() {
                        let mut data =
                            TursoBackend::parse_row_values_with_schema(&values, &columns);
                        data.insert("_rowid".into(), Value::String(change.id.to_string()));
                        let entity_id = data
                            .get("id")
                            .and_then(|v| match v {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            })
                            .unwrap_or_else(|| change.id.to_string());
                        let origin = extract_change_origin_from_data(&data);
                        if batch_trace_context.is_none() {
                            batch_trace_context = origin.to_batch_trace_context();
                        }
                        ChangeData::Deleted {
                            id: entity_id,
                            origin,
                        }
                    } else {
                        ChangeData::Deleted {
                            id: change.id.to_string(),
                            origin: ChangeOrigin::Remote {
                                operation_id: None,
                                trace_id: None,
                            },
                        }
                    }
                }
            };

            raw_changes.push(RowChange {
                relation_name: event.relation_name.clone(),
                change: change_data,
            });
        }

        let coalesced_changes = coalesce_row_changes(raw_changes);
        let batch = Batch {
            items: coalesced_changes,
        };
        let metadata = BatchMetadata {
            relation_name: event.relation_name.clone(),
            trace_context: batch_trace_context,
            sync_token: None,
            // Filled in by `set_change_callback` in `new_with_options` after
            // `process_cdc_event` returns — process-wide monotonic counter.
            seq: 0,
        };

        BatchWithMetadata {
            inner: batch,
            metadata,
        }
    }

    /// Internal actor loop - runs in spawned task
    async fn run_actor(
        mut rx: mpsc::Receiver<DbCommand>,
        conn: turso::Connection,
        cdc_broadcast: broadcast::Sender<BatchWithMetadata<RowChange>>,
        actor_stats: Option<Arc<crate::turso_actor_stats::ActorStats>>,
        matview_stats: Arc<MatviewStats>,
    ) {
        tracing::info!("[TursoBackend::Actor] Starting actor loop");

        let mut state = ActorState::new(matview_stats);

        while let Some(cmd) = rx.recv().await {
            let stats_meta = actor_stats.as_ref().map(|_| {
                let (variant, sql) = crate::turso_actor_stats::cmd_fingerprint(&cmd);
                (
                    variant,
                    sql.map(crate::turso_actor_stats::fingerprint_sql),
                    std::time::Instant::now(),
                )
            });

            // Wrap command processing in catch_unwind to prevent panics
            // (e.g., from tracing-subscriber span lifecycle bugs) from killing the actor.
            let should_break: std::result::Result<bool, Box<dyn std::any::Any + Send>> =
                AssertUnwindSafe(Self::process_actor_command(
                    cmd,
                    &conn,
                    &mut state,
                    &cdc_broadcast,
                ))
                .catch_unwind()
                .await;

            if let (Some(stats), Some((variant, sql_key, t0))) = (&actor_stats, stats_meta) {
                stats.record_command(variant, sql_key, t0.elapsed());
            }

            match should_break {
                Ok(true) => break,
                Ok(false) => {}
                Err(panic_info) => {
                    let msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = panic_info.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "unknown panic".to_string()
                    };
                    tracing::error!(
                        "[TursoBackend::Actor] Caught panic during command processing: {}. Actor \
                         continues.",
                        msg
                    );
                    // If a panic left a transaction open, roll it back to prevent
                    // the connection from being stuck (which silences CDC callbacks).
                    if !conn.is_autocommit().unwrap_or(true) {
                        tracing::error!(
                            "[TursoBackend::Actor] Connection stuck in transaction after panic, \
                             rolling back"
                        );
                        if let Err(e) = conn.execute("ROLLBACK", ()).await {
                            tracing::error!(
                                "[TursoBackend::Actor] Failed to rollback after panic: {}",
                                e
                            );
                        }
                    }
                }
            }
        }

        tracing::info!("[TursoBackend::Actor] Actor loop ended");
    }

    /// Process a single actor command. Returns true if the actor should shut
    /// down.
    async fn process_actor_command(
        cmd: DbCommand,
        conn: &turso::Connection,
        state: &mut ActorState,
        cdc_broadcast: &broadcast::Sender<BatchWithMetadata<RowChange>>,
    ) -> bool {
        match cmd {
            DbCommand::Query {
                sql,
                params,
                response,
            } => {
                trace_sql_named("actor_query", &sql, &params);
                let result = Self::handle_query(conn, &sql, params).await;
                let _ = response.send(result);
            }

            DbCommand::QueryPositional {
                sql,
                params,
                response,
            } => {
                trace_sql_positional("actor_query", &sql, &params);
                let result = Self::handle_query_positional(conn, &sql, params).await;
                let _ = response.send(result);
            }

            DbCommand::Execute {
                sql,
                params,
                response,
            } => {
                trace_sql_positional("actor_exec", &sql, &params);
                let result = Self::handle_execute(conn, &sql, params).await;
                let _ = response.send(result);
            }

            DbCommand::ExecuteDdl { sql, response } => {
                let result = Self::handle_ddl(conn, &sql).await;
                if result.is_ok()
                    && let Ok(stmts) = parse_sql(&sql)
                {
                    let provides = extract_created_tables(&stmts);
                    Self::mark_resources_available(&mut state.available_resources, &provides);
                    if !provides.is_empty() {
                        Self::process_pending_ddl(conn, state).await;
                    }
                }
                let _ = response.send(result);
            }

            DbCommand::ExecuteDdlWithDeps {
                sql,
                provides,
                requires,
                priority,
                response,
            } => {
                Self::handle_ddl_with_deps_internal(
                    conn,
                    state,
                    sql,
                    provides,
                    requires,
                    priority,
                    DdlCompletion::Caller(response),
                )
                .await;
            }

            DbCommand::ExecuteDdlAuto {
                sql,
                priority,
                response,
            } => {
                let stmts = parse_sql(&sql).unwrap_or_default();
                let provides = extract_created_tables(&stmts);
                let mut requires = extract_table_refs(&stmts);
                for provided in &provides {
                    requires.retain(|r| r != provided);
                }
                Self::handle_ddl_with_deps_internal(
                    conn,
                    state,
                    sql,
                    provides,
                    requires,
                    priority,
                    DdlCompletion::Caller(response),
                )
                .await;
            }

            DbCommand::MarkAvailable { resources } => {
                Self::mark_resources_available(&mut state.available_resources, &resources);
                Self::process_pending_ddl(conn, state).await;
            }

            DbCommand::ResourceExists { resource, response } => {
                let exists = state.available_resources.contains(&resource);
                let _ = response.send(exists);
            }

            DbCommand::Transaction {
                statements,
                response,
            } => {
                let result = Self::handle_transaction(conn, statements).await;
                let _ = response.send(result);
            }

            DbCommand::SubscribeCdc { relation, response } => {
                let rx = cdc_broadcast.subscribe();
                let _ = response.send(Ok(rx));
                tracing::debug!(
                    "[TursoBackend::Actor] CDC subscription created for relation: {}",
                    relation
                );
            }

            DbCommand::TransitionToReady { response } => {
                // Ready first: parking is legitimate while SchemaInit is open,
                // so the sweep is a no-op until the phase has flipped.
                state.phase = DatabasePhase::Ready;
                Self::sweep_unpromised_pending_ddl(state);
                tracing::info!("[TursoBackend::Actor] Transitioned to Ready phase");
                let _ = response.send(Ok(()));
            }

            DbCommand::GetPhase { response } => {
                let _ = response.send(state.phase);
            }

            DbCommand::RegisterForeignTable {
                name,
                fdw,
                response,
            } => {
                let result = conn.register_foreign_table(&name, fdw).map_err(|e| {
                    StorageError::DatabaseError(format!(
                        "Failed to register foreign table '{name}': {e}"
                    ))
                });
                if result.is_ok() {
                    tracing::info!("[TursoBackend::Actor] Registered foreign table '{name}'");
                    // A foreign table is a real dependency target: without this
                    // the availability registry would call it unregistered and
                    // fail every watch over it.
                    Self::mark_resources_available(
                        &mut state.available_resources,
                        &[Resource::schema(name)],
                    );
                    Self::process_pending_ddl(conn, state).await;
                }
                let _ = response.send(result);
            }

            DbCommand::AcquireViewLease {
                view_name,
                select_sql,
                requires,
                response,
            } => {
                Self::handle_view_waiter(
                    conn,
                    state,
                    view_name,
                    select_sql,
                    requires,
                    ViewWaiter::Lease(response),
                )
                .await;
            }

            DbCommand::EnsurePinnedView {
                view_name,
                select_sql,
                requires,
                response,
            } => {
                Self::handle_view_waiter(
                    conn,
                    state,
                    view_name,
                    select_sql,
                    requires,
                    ViewWaiter::Pin(response),
                )
                .await;
            }

            DbCommand::ReleaseViewLease {
                view_name,
                lease_id,
                generation,
            } => {
                Self::handle_release_view_lease(conn, state, &view_name, lease_id, generation)
                    .await;
            }

            DbCommand::ResetWatchViews { response } => {
                let result = Self::handle_reset_watch_views(conn, state).await;
                let _ = response.send(result);
            }

            DbCommand::Shutdown { response } => {
                state.phase = DatabasePhase::ShuttingDown;
                tracing::info!("[TursoBackend::Actor] Shutting down");
                let _ = response.send(());
                return true;
            }
        }
        false
    }

    /// Handle a query command
    pub(crate) async fn handle_query(
        conn: &turso::Connection,
        sql: &str,
        params: HashMap<String, Value>,
    ) -> Result<Vec<StorageEntity>> {
        // Bind named parameters to positional placeholders
        let (sql_with_placeholders, param_values) = bind_parameters(sql, &params)?;

        let mut stmt = conn
            .prepare(&sql_with_placeholders)
            .await
            .map_err(|e| StorageError::DatabaseError(format!("Failed to prepare query: {}", e)))?;

        // Convert column names to Arc<str> once per statement; rows below only
        // Arc::clone
        let col_names: Vec<Arc<str>> = stmt.columns().iter().map(|c| Arc::from(c.name())).collect();

        let mut rows = stmt
            .query(param_values)
            .await
            .map_err(|e| StorageError::QueryError(format!("Failed to execute query: {}", e)))?;

        let mut results = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| StorageError::QueryError(format!("Failed to fetch row: {}", e)))?
        {
            let mut entity = StorageEntity::with_capacity(col_names.len());

            for (idx, col_name) in col_names.iter().enumerate() {
                let value = row.get_value(idx).map_err(|e| {
                    StorageError::QueryError(format!("Failed to get column value: {}", e))
                })?;

                entity.insert(Arc::clone(col_name), turso_value_to_value(value.into()));
            }

            normalize_known_json_columns(&mut entity);

            results.push(entity);
        }

        Ok(results)
    }

    /// Handle a query command with positional parameters
    async fn handle_query_positional(
        conn: &turso::Connection,
        sql: &str,
        params: Vec<turso::Value>,
    ) -> Result<Vec<StorageEntity>> {
        let mut stmt = conn
            .prepare(sql)
            .await
            .map_err(|e| StorageError::DatabaseError(format!("Failed to prepare query: {}", e)))?;

        // Convert column names to Arc<str> once per statement; rows below only
        // Arc::clone
        let col_names: Vec<Arc<str>> = stmt.columns().iter().map(|c| Arc::from(c.name())).collect();

        let mut rows = stmt
            .query(params)
            .await
            .map_err(|e| StorageError::QueryError(format!("Failed to execute query: {}", e)))?;

        let mut results = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| StorageError::QueryError(format!("Failed to fetch row: {}", e)))?
        {
            let mut entity = StorageEntity::with_capacity(col_names.len());

            for (idx, col_name) in col_names.iter().enumerate() {
                let value = row.get_value(idx).map_err(|e| {
                    StorageError::QueryError(format!("Failed to get column value: {}", e))
                })?;

                entity.insert(Arc::clone(col_name), turso_value_to_value(value.into()));
            }

            normalize_known_json_columns(&mut entity);

            results.push(entity);
        }

        Ok(results)
    }

    /// Handle an execute command
    async fn handle_execute(
        conn: &turso::Connection,
        sql: &str,
        params: Vec<turso::Value>,
    ) -> Result<u64> {
        let mut stmt = conn.prepare(sql).await.map_err(|e| {
            StorageError::DatabaseError(format!("Failed to prepare statement: {}", e))
        })?;

        let rows_affected = stmt.execute(params).await.map_err(|e| {
            StorageError::DatabaseError(format!("Failed to execute statement: {}", e))
        })?;

        Ok(rows_affected)
    }

    /// Handle a DDL command
    pub(crate) async fn handle_ddl(conn: &turso::Connection, sql: &str) -> Result<()> {
        trace_sql("actor_ddl", sql);
        // Latency stage (matview/read-path maintenance): a `CREATE MATERIALIZED
        // VIEW watch_view_*` cold-materializes here on page navigation and can
        // take seconds (recursive IVM warm-up). This is the choke point every
        // watch-view maintenance passes through — one greppable line per DDL,
        // so a 12s cold materialization is visible in a log. Greppable via
        // target="holon_latency".
        let t_ddl = std::time::Instant::now();

        // The actor processes commands sequentially, so an unbounded DDL await
        // parks the *entire* DB actor: every later query/exec queues behind it
        // and never gets a response — the app freezes with no error. A
        // `CREATE MATERIALIZED VIEW` that selects FROM another matview can hang
        // forever in Turso IVM (matview-on-matview is unsupported — see
        // .claude/skills/turso-chained-matview-hang/SKILL.md). Bounding the
        // execution here lets the actor recover and surface a loud error
        // instead. The caller-side DEPENDENCY_TIMEOUT (120s) does NOT help:
        // it only abandons the caller's wait; the actor stays parked forever.
        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        {
            let timeout = Self::ddl_execution_timeout();
            match tokio::time::timeout(timeout, conn.execute(sql, ())).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    return Err(StorageError::DatabaseError(format!(
                        "Failed to execute DDL: {}",
                        e
                    )));
                }
                Err(_elapsed) => {
                    let sql_preview: String = sql.chars().take(160).collect();
                    return Err(StorageError::DatabaseError(format!(
                        "DDL execution timed out after {:?} — actor would have hung. Suspected \
                         Turso chained-matview (matview-on-matview) limitation: CREATE \
                         MATERIALIZED VIEW selecting FROM another matview hangs indefinitely in \
                         Turso IVM. See .claude/skills/turso-chained-matview-hang/SKILL.md. SQL: \
                         {}...",
                        timeout, sql_preview
                    )));
                }
            }
        }
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        {
            conn.execute(sql, ()).await.map_err(|e| {
                StorageError::DatabaseError(format!("Failed to execute DDL: {}", e))
            })?;
        }

        tracing::info!(
            target: "holon_latency",
            stage = "matview_ddl",
            view = ddl_target_name(sql),
            ms = t_ddl.elapsed().as_millis() as u64,
            "holon_latency",
        );
        tracing::debug!("[TursoBackend::Actor] DDL completed successfully");
        Ok(())
    }

    /// Upper bound on a single DDL statement's execution inside the actor.
    ///
    /// Kept well under the caller-side `DEPENDENCY_TIMEOUT` (120s) so a genuine
    /// hang is caught here first (freeing the actor) rather than only
    /// abandoning one caller's wait. Overridable via `HOLON_DDL_TIMEOUT_MS`
    /// so tests can exercise the hang guard without a 30s wall.
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    fn ddl_execution_timeout() -> std::time::Duration {
        const DDL_EXECUTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
        match std::env::var("HOLON_DDL_TIMEOUT_MS") {
            Ok(ms) => std::time::Duration::from_millis(
                ms.parse()
                    .expect("HOLON_DDL_TIMEOUT_MS must be a u64 millisecond count"),
            ),
            Err(_) => DDL_EXECUTION_TIMEOUT,
        }
    }

    /// Handle a transaction command
    async fn handle_transaction(
        conn: &turso::Connection,
        statements: Vec<(String, Vec<turso::Value>)>,
    ) -> Result<()> {
        tracing::trace!(
            "[TursoBackend] actor_tx_begin: BEGIN TRANSACTION ({} stmts)",
            statements.len()
        );
        // Begin transaction — if the connection is stuck in a stale transaction
        // (e.g., from a previous commit failure or panic), rollback and retry.
        if let Err(e) = conn.execute("BEGIN TRANSACTION", ()).await {
            if !conn.is_autocommit().unwrap_or(true) {
                tracing::warn!(
                    "[TursoBackend::Actor] BEGIN failed with stale transaction, rolling back and \
                     retrying: {}",
                    e
                );
                if let Err(rollback_err) = conn.execute("ROLLBACK", ()).await {
                    tracing::warn!(
                        "[TursoBackend::Actor] ROLLBACK after stale BEGIN failed: {}",
                        rollback_err
                    );
                }
                conn.execute("BEGIN TRANSACTION", ()).await.map_err(|e| {
                    StorageError::DatabaseError(format!(
                        "Failed to begin transaction after rollback: {}",
                        e
                    ))
                })?;
            } else {
                return Err(StorageError::DatabaseError(format!(
                    "Failed to begin transaction: {}",
                    e
                )));
            }
        }

        // Execute each statement, rolling back on any error
        let result = Self::execute_statements_in_transaction(conn, statements).await;

        if result.is_err() {
            // Rollback on error
            if let Err(rollback_err) = conn.execute("ROLLBACK", ()).await {
                tracing::error!(
                    "[TursoBackend::Actor] Failed to rollback transaction: {}",
                    rollback_err
                );
            }
            return result;
        }

        // Commit transaction
        tracing::trace!("[TursoBackend] actor_tx_commit: COMMIT");
        if let Err(e) = conn.execute("COMMIT", ()).await {
            tracing::error!("[TursoBackend::Actor] Commit failed, rolling back: {}", e);
            if let Err(rollback_err) = conn.execute("ROLLBACK", ()).await {
                tracing::error!(
                    "[TursoBackend::Actor] Rollback after failed commit also failed: {}",
                    rollback_err
                );
            }
            return Err(StorageError::DatabaseError(format!(
                "Failed to commit transaction: {}",
                e
            )));
        }

        Ok(())
    }

    /// Execute statements within a transaction (helper for proper error
    /// handling)
    async fn execute_statements_in_transaction(
        conn: &turso::Connection,
        statements: Vec<(String, Vec<turso::Value>)>,
    ) -> Result<()> {
        for (sql, params) in statements {
            trace_sql_positional("transaction_stmt", &sql, &params);
            let mut stmt = conn.prepare(&sql).await.map_err(|e| {
                StorageError::DatabaseError(format!("Failed to prepare statement: {}", e))
            })?;

            stmt.execute(params).await.map_err(|e| {
                StorageError::DatabaseError(format!("Failed to execute statement: {}", e))
            })?;
        }
        Ok(())
    }

    // --- Dependency tracking methods ---

    /// Mark resources as available and log them
    fn mark_resources_available(
        available_resources: &mut HashSet<Resource>,
        resources: &[Resource],
    ) {
        for resource in resources {
            available_resources.insert(resource.clone());
        }
        if !resources.is_empty() {
            tracing::debug!(
                "[TursoBackend::Actor] Marked {} resources as available: {:?}",
                resources.len(),
                resources.iter().map(|r| r.name()).collect::<Vec<_>>()
            );
        }
    }

    /// Check if all required resources are available
    fn can_execute_ddl(available_resources: &HashSet<Resource>, op: &PendingDdl) -> bool {
        op.requires.iter().all(|r| available_resources.contains(r))
    }

    /// Requirements of a not-yet-runnable `op` that nobody will ever satisfy,
    /// or `None` when parking is legitimate.
    ///
    /// Parking is legitimate while `SchemaInit` is open — DI resolves schema
    /// providers in parallel, so a base table can arrive after the matview that
    /// selects from it was submitted. Once `Ready`, registration is closed: the
    /// only outstanding promises are the `provides` of DDL already queued.
    fn unpromised_requirements(state: &ActorState, op: &PendingDdl) -> Option<Vec<String>> {
        if state.phase == DatabasePhase::SchemaInit {
            return None;
        }
        let unpromised: Vec<String> = op
            .requires
            .iter()
            .filter(|r| !state.available_resources.contains(r))
            .filter(|r| {
                !state
                    .pending_ddl
                    .iter()
                    .any(|queued| queued.provides.contains(r))
            })
            .map(|r| r.name().to_string())
            .collect();
        (!unpromised.is_empty()).then_some(unpromised)
    }

    /// Answer `op`'s waiter with `MissingDependencies` instead of letting it
    /// wait out the dependency timeout.
    fn fail_unpromised_ddl(state: &mut ActorState, op: PendingDdl, unpromised: Vec<String>) {
        let sql_preview: String = sql_fingerprint(&op.sql);
        tracing::warn!(
            missing = ?unpromised,
            sql = %sql_preview,
            "[TursoBackend::Actor] DDL requires resources no schema provider registers — failing \
             it instead of waiting"
        );
        let err = StorageError::MissingDependencies {
            sql_preview,
            missing: unpromised,
        };
        match op.completion {
            DdlCompletion::Caller(response) => {
                let _ = response.send(Err(err));
            }
            DdlCompletion::ViewCreate { view_name } => {
                Self::finish_view_creation(state, &view_name, Err(err));
            }
        }
    }

    /// Fail every parked op whose requirements nobody will ever satisfy.
    ///
    /// Called once registration has closed. Failing one op can strip the last
    /// promise from another, so this runs to a fixpoint.
    fn sweep_unpromised_pending_ddl(state: &mut ActorState) {
        while let Some(index) = state
            .pending_ddl
            .iter()
            .position(|op| Self::unpromised_requirements(state, op).is_some())
        {
            let op = state.pending_ddl.remove(index).expect("index just found");
            let unpromised =
                Self::unpromised_requirements(state, &op).expect("op selected as unpromised");
            Self::fail_unpromised_ddl(state, op, unpromised);
        }
    }

    /// Handle DDL with dependency tracking
    async fn handle_ddl_with_deps_internal(
        conn: &turso::Connection,
        state: &mut ActorState,
        sql: String,
        provides: Vec<Resource>,
        requires: Vec<Resource>,
        priority: u32,
        completion: DdlCompletion,
    ) {
        let op_id = state.next_op_id();

        let op = PendingDdl {
            id: op_id,
            sql,
            provides,
            requires,
            priority,
            completion,
        };

        // Check if we can execute immediately
        if Self::can_execute_ddl(&state.available_resources, &op) {
            let had_provides = !op.provides.is_empty();
            Self::execute_pending_ddl(conn, state, op).await;
            // Resources this DDL provided may unblock already-queued ops.
            if had_provides {
                Self::process_pending_ddl(conn, state).await;
            }
        } else if let Some(unpromised) = Self::unpromised_requirements(state, &op) {
            Self::fail_unpromised_ddl(state, op, unpromised);
        } else {
            tracing::debug!(
                "[TursoBackend::Actor] DDL op {} queued, waiting for: {:?}",
                op_id,
                op.requires
                    .iter()
                    .filter(|r| !state.available_resources.contains(r))
                    .map(|r| r.name())
                    .collect::<Vec<_>>()
            );
            state.pending_ddl.push_back(op);
        }
    }

    /// Execute a pending DDL operation
    async fn execute_pending_ddl(conn: &turso::Connection, state: &mut ActorState, op: PendingDdl) {
        tracing::debug!("[TursoBackend::Actor] Executing DDL op {}", op.id);

        let PendingDdl {
            sql,
            provides,
            completion,
            ..
        } = op;
        let result = Self::handle_ddl(conn, &sql).await;

        if result.is_ok() {
            // Mark provided resources as available
            Self::mark_resources_available(&mut state.available_resources, &provides);
        }

        match completion {
            DdlCompletion::Caller(response) => {
                let _ = response.send(result);
            }
            DdlCompletion::ViewCreate { view_name } => {
                Self::finish_view_creation(state, &view_name, result);
            }
        }
    }

    /// Process pending DDL operations that may now be ready
    async fn process_pending_ddl(conn: &turso::Connection, state: &mut ActorState) {
        // Collect ready operations
        let mut ready = Vec::new();
        let mut still_pending = VecDeque::new();

        while let Some(op) = state.pending_ddl.pop_front() {
            if Self::can_execute_ddl(&state.available_resources, &op) {
                ready.push(op);
            } else {
                still_pending.push_back(op);
            }
        }

        state.pending_ddl = still_pending;

        // Sort by priority (highest first)
        ready.sort_by_key(|op| std::cmp::Reverse(op.priority));

        // Execute ready operations
        for op in ready {
            Self::execute_pending_ddl(conn, state, op).await;
            // After each execution, more ops may become ready
            // Recursively process (this is safe since we drain the queue)
        }

        // Recursively check if new ops are now ready
        if state
            .pending_ddl
            .iter()
            .any(|op| Self::can_execute_ddl(&state.available_resources, op))
        {
            Box::pin(Self::process_pending_ddl(conn, state)).await;
        }
    }

    // ========================================================================
    // Matview lease lifecycle
    // ========================================================================

    /// Serve one `AcquireViewLease`/`EnsurePinnedView`: grant against a `Live`
    /// view, park behind an in-flight `CREATE`, or start the `CREATE`.
    async fn handle_view_waiter(
        conn: &turso::Connection,
        state: &mut ActorState,
        view_name: String,
        select_sql: String,
        requires: Vec<Resource>,
        waiter: ViewWaiter,
    ) {
        let generation = state.generation;
        let lease_id = state.next_lease_id();

        // Taken by whichever branch below serves the waiter; what is left over
        // is what the create path must park.
        let mut unserved = Some(waiter);
        match state.views.get_mut(&view_name) {
            Some(ViewState::Creating {
                waiters,
                pin_requested,
            }) => {
                let waiter = unserved.take().expect("waiter is served exactly once");
                *pin_requested |= waiter.is_pin();
                waiters.push(waiter);
            }
            Some(ViewState::Live { leases, pinned }) => {
                match unserved.take().expect("waiter is served exactly once") {
                    ViewWaiter::Lease(response) => {
                        *leases += 1;
                        let _ = response.send(Ok(LeaseGrant {
                            lease_id,
                            generation,
                        }));
                    }
                    ViewWaiter::Pin(response) => {
                        *pinned = true;
                        let _ = response.send(Ok(()));
                    }
                }
            }
            None => {}
        }

        let Some(waiter) = unserved else {
            state.publish_matview_stats();
            return;
        };

        let pin_requested = waiter.is_pin();
        state.views.insert(
            view_name.clone(),
            ViewState::Creating {
                waiters: vec![waiter],
                pin_requested,
            },
        );
        state.publish_matview_stats();

        if let Err(e) =
            crate::matview_manager::cleanup_orphaned_dbsp_state_on_conn(conn, &view_name).await
        {
            Self::finish_view_creation(
                state,
                &view_name,
                Err(StorageError::DatabaseError(format!(
                    "matview '{view_name}': could not inspect residual DBSP state before its \
                     CREATE: {e}"
                ))),
            );
            return;
        }

        let create_sql =
            format!("CREATE MATERIALIZED VIEW IF NOT EXISTS {view_name} AS {select_sql}");
        let provides = vec![Resource::schema(view_name.clone())];
        Self::handle_ddl_with_deps_internal(
            conn,
            state,
            create_sql,
            provides,
            requires,
            priority::DDL_MATVIEW,
            DdlCompletion::ViewCreate { view_name },
        )
        .await;
    }

    /// Answer everyone parked on a view whose `CREATE` just finished.
    fn finish_view_creation(state: &mut ActorState, view_name: &str, result: Result<()>) {
        let Some(ViewState::Creating {
            waiters,
            pin_requested,
        }) = state.views.remove(view_name)
        else {
            tracing::error!(
                view = %view_name,
                "[TursoBackend::Actor] matview CREATE completed for a view that is not in the \
                 Creating state — lease bookkeeping bug; its waiters will never be answered"
            );
            return;
        };

        let error = match result {
            Ok(()) => None,
            Err(e) => Some(format!("matview '{view_name}' could not be created: {e}")),
        };
        if let Some(message) = error {
            tracing::error!("[TursoBackend::Actor] {message}");
            for waiter in waiters {
                waiter.fail(message.clone());
            }
            state.publish_matview_stats();
            return;
        }

        let generation = state.generation;
        let mut leases = 0u32;
        for waiter in waiters {
            match waiter {
                ViewWaiter::Lease(response) => {
                    leases += 1;
                    let lease_id = state.next_lease_id();
                    let _ = response.send(Ok(LeaseGrant {
                        lease_id,
                        generation,
                    }));
                }
                ViewWaiter::Pin(response) => {
                    let _ = response.send(Ok(()));
                }
            }
        }
        state.views.insert(
            view_name.to_string(),
            ViewState::Live {
                leases,
                pinned: pin_requested,
            },
        );
        state.publish_matview_stats();
    }

    /// Give back one lease and, if it was the last reason to keep the view,
    /// reap it here and now — the release and the drop are one command.
    async fn handle_release_view_lease(
        conn: &turso::Connection,
        state: &mut ActorState,
        view_name: &str,
        lease_id: u64,
        generation: u64,
    ) {
        if generation != state.generation {
            tracing::debug!(
                view = %view_name,
                lease_id,
                grant_generation = generation,
                current_generation = state.generation,
                "[TursoBackend::Actor] discarding a matview lease release from a bygone \
                 generation — its view was already dropped by a reset"
            );
            return;
        }

        let reap = match state.views.get_mut(view_name) {
            Some(ViewState::Live { leases, pinned }) => {
                if *leases == 0 {
                    tracing::error!(
                        view = %view_name,
                        lease_id,
                        "[TursoBackend::Actor] matview lease released twice — lease bookkeeping \
                         bug; ignoring so the count cannot underflow"
                    );
                    return;
                }
                *leases -= 1;
                *leases == 0 && !*pinned
            }
            Some(ViewState::Creating { .. }) => {
                tracing::error!(
                    view = %view_name,
                    lease_id,
                    "[TursoBackend::Actor] matview lease released while its view is still being \
                     created — no grant can exist yet, so this is a lease bookkeeping bug"
                );
                return;
            }
            None => {
                tracing::error!(
                    view = %view_name,
                    lease_id,
                    "[TursoBackend::Actor] matview lease released for a view the actor does not \
                     own — lease bookkeeping bug"
                );
                return;
            }
        };

        if !reap {
            state.publish_matview_stats();
            return;
        }
        Self::reap_view(conn, state, view_name).await;
    }

    /// Drop an unleased, unpinned view together with its dependents.
    async fn reap_view(conn: &turso::Connection, state: &mut ActorState, view_name: &str) {
        let dependents =
            match crate::matview_manager::dependent_views_on_conn(conn, view_name).await {
                Ok(dependents) => dependents,
                Err(e) => {
                    tracing::error!(
                        view = %view_name,
                        "[TursoBackend::Actor] cannot enumerate dependents of an unleased \
                         matview, so it stays materialized (its DBSP circuit keeps costing every \
                         commit): {e}"
                    );
                    return;
                }
            };

        let blocked: Vec<&String> = dependents.iter().filter(|d| state.is_held(d)).collect();
        if !blocked.is_empty() {
            tracing::error!(
                view = %view_name,
                ?blocked,
                "[TursoBackend::Actor] refusing to reap an unleased matview: dependent matviews \
                 still hold live leases and dropping their base would leave them reading a view \
                 that no longer exists"
            );
            return;
        }

        let mut doomed = dependents;
        doomed.push(view_name.to_string());
        for name in &doomed {
            if let Err(e) = Self::handle_ddl(conn, &format!("DROP VIEW IF EXISTS {name}")).await {
                tracing::error!(
                    view = %name,
                    "[TursoBackend::Actor] failed to drop an unleased matview; it stays \
                     materialized: {e}"
                );
                return;
            }
            if let Err(e) =
                crate::matview_manager::cleanup_orphaned_dbsp_state_on_conn(conn, name).await
            {
                tracing::warn!(
                    view = %name,
                    "[TursoBackend::Actor] could not inspect DBSP residue after dropping a \
                     matview: {e}"
                );
            }
            state.views.remove(name);
            state
                .available_resources
                .remove(&Resource::schema(name.clone()));
        }
        // Anyone caching "this view exists" is now wrong.
        state.matview_stats.note_reap();
        state.publish_matview_stats();
        tracing::debug!(view = %view_name, "[TursoBackend::Actor] reaped unleased matview");
    }

    /// Drop every `watch_view_%` and start a new lease generation.
    async fn handle_reset_watch_views(
        conn: &turso::Connection,
        state: &mut ActorState,
    ) -> Result<usize> {
        let rows = Self::handle_query(
            conn,
            &format!(
                "SELECT name FROM sqlite_master WHERE type='view' AND name LIKE '{}%'",
                crate::matview_manager::WATCH_VIEW_PREFIX
            ),
            HashMap::new(),
        )
        .await?;

        let mut dropped = 0usize;
        for row in &rows {
            let Some(Value::String(name)) = row.get("name") else {
                continue;
            };
            Self::handle_ddl(conn, &format!("DROP VIEW IF EXISTS {name}")).await?;
            crate::matview_manager::cleanup_orphaned_dbsp_state_on_conn(conn, name)
                .await
                .map_err(|e| {
                    StorageError::DatabaseError(format!(
                        "reset_watch_views: DBSP residue check for '{name}' failed: {e}"
                    ))
                })?;
            state
                .available_resources
                .remove(&Resource::schema(name.clone()));
            dropped += 1;
        }

        // A queued `CREATE` for this epoch would materialize a view nobody can
        // ever be told about, so drop those ops with their views.
        state
            .pending_ddl
            .retain(|op| !matches!(op.completion, DdlCompletion::ViewCreate { .. }));

        // Everything parked belongs to the epoch we are ending; a waiter left
        // hanging would block its caller for the full dependency timeout.
        for (name, view_state) in state.views.drain() {
            if let ViewState::Creating { waiters, .. } = view_state {
                for waiter in waiters {
                    waiter.fail(format!(
                        "matview '{name}': its CREATE was abandoned by a watch-view reset"
                    ));
                }
            }
        }
        state.generation += 1;
        state.matview_stats.note_reap();
        state.publish_matview_stats();

        if dropped > 0 {
            tracing::info!(
                "[TursoBackend::Actor] reset dropped {dropped} watch views; lease generation is \
                 now {}",
                state.generation
            );
        }
        Ok(dropped)
    }
}

#[async_trait]
impl StorageBackend for TursoBackend {
    async fn create_entity(&self, type_def: &holon_api::TypeDefinition) -> Result<()> {
        let create_sql = type_def.to_create_table_sql();
        self.handle().execute_ddl(&create_sql).await?;

        for index_sql in type_def.to_index_sql() {
            self.handle().execute_ddl(&index_sql).await?;
        }

        Ok(())
    }

    async fn get(&self, entity: &str, id: &str) -> Result<Option<StorageEntity>> {
        let query_str = format!("SELECT * FROM {} WHERE id = $id", entity);
        let mut params = HashMap::new();
        params.insert("id".to_string(), Value::String(id.to_string()));
        let results = self.handle().query(&query_str, params).await?;
        Ok(results.into_iter().next())
    }

    async fn query(&self, entity: &str, filter: Filter) -> Result<Vec<StorageEntity>> {
        let mut params = Vec::new();
        let where_clause = self.build_where_clause(&filter, &mut params);
        let query_str = format!("SELECT * FROM {} WHERE {}", entity, where_clause);
        self.handle().query_positional(&query_str, params).await
    }

    async fn insert(&self, schema: &holon_api::TypeDefinition, data: StorageEntity) -> Result<()> {
        let fields: Vec<_> = data.keys().collect();
        let placeholders: Vec<String> = fields
            .iter()
            .map(|f| {
                if schema.field_is_jsonb(f) {
                    "jsonb(?)".to_string()
                } else {
                    "?".to_string()
                }
            })
            .collect();

        let insert_sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            schema.name,
            fields
                .iter()
                .map(|f| f.as_ref())
                .collect::<Vec<_>>()
                .join(", "),
            placeholders.join(", ")
        );

        let params: Vec<turso::Value> = data.values().map(value_to_turso_param).collect();

        self.handle().execute(&insert_sql, params).await?;
        Ok(())
    }

    async fn update(
        &self,
        schema: &holon_api::TypeDefinition,
        id: &str,
        data: StorageEntity,
    ) -> Result<()> {
        let filtered_data: Vec<_> = data.iter().filter(|(k, _)| &***k != "id").collect();

        let set_clauses: Vec<String> = filtered_data
            .iter()
            .map(|(k, _)| {
                if schema.field_is_jsonb(k) {
                    format!("{} = jsonb(?)", k)
                } else {
                    format!("{} = ?", k)
                }
            })
            .collect();

        let update_sql = format!(
            "UPDATE {} SET {} WHERE id = ?",
            schema.name,
            set_clauses.join(", ")
        );

        let mut params: Vec<turso::Value> = filtered_data
            .iter()
            .map(|(_, v)| value_to_turso_param(v))
            .collect();
        params.push(turso::Value::Text(id.to_string()));

        self.handle().execute(&update_sql, params).await?;
        Ok(())
    }

    async fn delete(&self, entity: &str, id: &str) -> Result<()> {
        let delete_sql = format!("DELETE FROM {} WHERE id = ?", entity);
        let params = vec![turso::Value::Text(id.to_string())];
        self.handle().execute(&delete_sql, params).await?;
        Ok(())
    }

    async fn get_version(&self, entity: &str, id: &str) -> Result<Option<String>> {
        let query = format!("SELECT _version FROM {} WHERE id = ?", entity);
        let params = vec![turso::Value::Text(id.to_string())];
        let results = self.handle().query_positional(&query, params).await?;
        if let Some(row) = results.into_iter().next() {
            return match row.get("_version") {
                Some(Value::String(s)) => Ok(Some(s.clone())),
                Some(Value::Null) | None => Ok(None),
                _ => Ok(None),
            };
        }
        Ok(None)
    }

    async fn set_version(&self, entity: &str, id: &str, version: String) -> Result<()> {
        let update_sql = format!("UPDATE {} SET _version = ? WHERE id = ?", entity);
        let params = vec![
            turso::Value::Text(version.clone()),
            turso::Value::Text(id.to_string()),
        ];
        self.handle().execute(&update_sql, params).await?;
        Ok(())
    }

    async fn get_children(
        &self,
        entity: &str,
        parent_field: &str,
        parent_id: &str,
    ) -> Result<Vec<StorageEntity>> {
        let filter = Filter::Eq(
            parent_field.to_string(),
            Value::String(parent_id.to_string()),
        );
        self.query(entity, filter).await
    }

    async fn get_related(
        &self,
        entity: &str,
        foreign_key: &str,
        related_id: &str,
    ) -> Result<Vec<StorageEntity>> {
        let filter = Filter::Eq(
            foreign_key.to_string(),
            Value::String(related_id.to_string()),
        );
        self.query(entity, filter).await
    }
}

// ============================================================================
// Tests
// ============================================================================

// NOTE: the heavier engine test suites (turso_tests, turso_pbt_tests,
// turso_matview_test, turso_ivm_join_test) live in the `holon` crate's
// `storage` module and exercise this engine through `holon::storage::turso`
// (re-export), because they pull in holon-side proptest fixtures and helpers.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_phase_default() {
        let phase = DatabasePhase::default();
        assert_eq!(phase, DatabasePhase::SchemaInit);
    }

    #[test]
    fn named_params_fingerprint_empty_is_dash_and_discriminates() {
        assert_eq!(named_params_fingerprint(&HashMap::new()), "-");
        let mut a = HashMap::new();
        a.insert("k".to_string(), Value::Integer(1));
        let mut b = HashMap::new();
        b.insert("k".to_string(), Value::Integer(2));
        assert_ne!(named_params_fingerprint(&a), "-");
        assert_ne!(named_params_fingerprint(&a), named_params_fingerprint(&b));
        assert_eq!(
            named_params_fingerprint(&a),
            named_params_fingerprint(&a.clone())
        );
    }

    // Head+tail truncation MERGES: two statements sharing a long prefix and a
    // long suffix but differing only in the middle collapsed into one bucket,
    // and the dedup gate then over-subtracted their combined excess. The
    // identity suffix makes the fingerprint injective in the SQL text, so
    // distinct statements can never share a bucket.
    #[test]
    fn sql_fingerprint_splits_statements_that_share_head_and_tail() {
        let head = format!(
            "WITH RECURSIVE d(id, parent_id, depth) AS ({})",
            "x".repeat(300)
        );
        let tail = format!("{} ORDER BY depth, sort_key", "y".repeat(300));
        let bare = format!("{head} SELECT id, parent_id FROM blocks {tail}");
        let full = format!("{head} SELECT id, parent_id, content, refs FROM blocks {tail}");

        assert_ne!(bare, full);
        assert_ne!(
            sql_fingerprint(&bare),
            sql_fingerprint(&full),
            "statements differing only in the truncated middle must not share a bucket"
        );
        assert_eq!(
            sql_fingerprint(&bare),
            sql_fingerprint(&bare.clone()),
            "the fingerprint is stable for one text"
        );
        assert!(
            sql_fingerprint(&bare).contains("WITH RECURSIVE"),
            "the readable head survives: {}",
            sql_fingerprint(&bare)
        );
    }

    // Same length, same head, same tail — only the middle bytes differ. A
    // length-only discriminator would still merge these.
    #[test]
    fn sql_fingerprint_splits_equal_length_statements() {
        let head = format!("SELECT {}", "a".repeat(300));
        let tail = "z".repeat(300);
        let left = format!("{head} FROM lhs_table_name {tail}");
        let right = format!("{head} FROM rhs_table_name {tail}");
        assert_eq!(left.len(), right.len());
        assert_ne!(sql_fingerprint(&left), sql_fingerprint(&right));
    }

    #[test]
    fn positional_params_fingerprint_empty_is_dash_and_discriminates() {
        assert_eq!(positional_params_fingerprint(&[]), "-");
        let a = [value_to_turso_param(&Value::Integer(1))];
        let b = [value_to_turso_param(&Value::Integer(2))];
        assert_ne!(positional_params_fingerprint(&a), "-");
        assert_ne!(
            positional_params_fingerprint(&a),
            positional_params_fingerprint(&b)
        );
    }

    // `$name` binding is injection-adjacent: the name-char predicate decides
    // where a placeholder ends. Underscore must be part of the name (kills the
    // `|| next == '_'` -> `&&` / `== '_'` -> `!= '_'` mutants on both the peek
    // and the inner scan), and a `$` not followed by a name char stays literal.
    #[test]
    fn bind_parameters_treats_underscore_as_name_char() {
        let mut params = HashMap::new();
        params.insert("foo_bar".to_string(), Value::Integer(7));
        let (sql, vals) = bind_parameters("SELECT $foo_bar", &params).expect("bind");
        assert_eq!(sql, "SELECT ?");
        assert_eq!(vals.len(), 1);
    }

    #[test]
    fn bind_parameters_leading_underscore_name() {
        let mut params = HashMap::new();
        params.insert("_priv".to_string(), Value::Integer(1));
        let (sql, _) = bind_parameters("SELECT $_priv", &params).expect("bind");
        assert_eq!(sql, "SELECT ?");
    }

    #[test]
    fn bind_parameters_bare_dollar_is_literal() {
        let (sql, vals) = bind_parameters("cost $ 5", &HashMap::new()).expect("bind");
        assert_eq!(sql, "cost $ 5");
        assert!(vals.is_empty());
    }

    // `:name` is the style an agent (and the MCP `params` map) writes. Leaving
    // it unbound is not inert: SQLite reads the unbound placeholder as NULL, so
    // the query succeeds and returns nothing.
    #[test]
    fn bind_parameters_binds_colon_and_at_styles() {
        for sigil in [':', '@'] {
            let mut params = HashMap::new();
            params.insert("pid".to_string(), Value::String("block:1820f890".into()));
            let (sql, vals) = bind_parameters(
                &format!("SELECT * FROM block WHERE parent_id = {sigil}pid"),
                &params,
            )
            .expect("bind");
            assert_eq!(
                sql, "SELECT * FROM block WHERE parent_id = ?",
                "sigil {sigil}"
            );
            assert!(
                matches!(vals.as_slice(), [turso::Value::Text(t)] if t == "block:1820f890"),
                "sigil {sigil} bound {vals:?}"
            );
        }
    }

    #[test]
    fn bind_parameters_unbound_colon_param_is_an_error_not_an_empty_result() {
        let err = bind_parameters(
            "SELECT * FROM block WHERE parent_id = :pid",
            &HashMap::new(),
        )
        .expect_err("an unbound placeholder must fail loud");
        let msg = format!("{err}");
        assert!(msg.contains("pid"), "error must name the parameter: {msg}");
    }

    // Schemed ids are colon-bearing, so the literal form of the very query the
    // named form failed on must keep working — and bind nothing.
    #[test]
    fn bind_parameters_schemed_id_literal_is_not_a_placeholder() {
        let sql = "SELECT * FROM block WHERE parent_id = 'block:1820f890-aaaa'";
        let (out, vals) = bind_parameters(sql, &HashMap::new()).expect("bind");
        assert_eq!(out, sql);
        assert!(vals.is_empty());
    }

    #[test]
    fn test_parse_json_object_object() {
        let mut obj = HashMap::new();
        obj.insert("key1".to_string(), Value::String("value1".to_string()));
        obj.insert("key2".to_string(), Value::Integer(42));

        let result = parse_json_object(Value::Object(obj.clone()));
        assert!(result.is_some());
        let parsed = result.unwrap();
        assert_eq!(
            parsed.get("key1"),
            Some(&Value::String("value1".to_string()))
        );
        assert_eq!(parsed.get("key2"), Some(&Value::Integer(42)));
    }

    #[test]
    fn test_parse_json_object_json_string() {
        let json_str = r#"{"key1": "value1", "key2": 42}"#;
        let result = parse_json_object(Value::String(json_str.to_string()));
        assert!(result.is_some());
        let parsed = result.unwrap();
        assert_eq!(
            parsed.get("key1"),
            Some(&Value::String("value1".to_string()))
        );
    }

    #[test]
    fn test_parse_json_object_non_json() {
        let result = parse_json_object(Value::String("not json".to_string()));
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_json_object_null() {
        let result = parse_json_object(Value::Null);
        assert!(result.is_none());
    }

    #[test]
    fn test_turso_value_to_value_conversions() {
        assert_eq!(turso_value_to_value(turso_core::Value::Null), Value::Null);
        assert_eq!(
            turso_value_to_value(turso_core::Value::from_i64(42)),
            Value::Integer(42)
        );
        assert_eq!(
            turso_value_to_value(turso_core::Value::from_f64(2.5)),
            Value::Float(2.5)
        );

        // Plain string
        let text_val = turso_value_to_value(turso_core::Value::Text("hello".into()));
        assert_eq!(text_val, Value::String("hello".to_string()));

        // User text that merely LOOKS like JSON stays a String — no content
        // sniffing (the CDC path never parsed it, so parsing here made the
        // two paths disagree about the same row).
        let arr_val = turso_value_to_value(turso_core::Value::Text("[1, 2, 3]".into()));
        assert_eq!(arr_val, Value::String("[1, 2, 3]".to_string()));
        let obj_val = turso_value_to_value(turso_core::Value::Text("{\"a\": 1}".into()));
        assert_eq!(obj_val, Value::String("{\"a\": 1}".to_string()));
    }

    /// Both row-parsing paths must agree: JSON-shaped user TEXT stays a
    /// String, while the known JSON columns (`data`, `properties`) come back
    /// structured. The query-path half of this lives in
    /// `integration_tests::test_json_shaped_text_round_trips_as_string`.
    #[test]
    fn test_cdc_path_keeps_json_shaped_text_as_string() {
        let values = vec![
            turso_core::Value::Text("b1".into()),
            turso_core::Value::Text("[1, 2, 3]".into()),
            turso_core::Value::Text(r#"{"k": "v"}"#.into()),
        ];
        let columns: Vec<Arc<str>> = vec![
            Arc::from("id"),
            Arc::from("content"),
            Arc::from("properties"),
        ];

        let entity = TursoBackend::parse_row_values_with_schema(&values, &columns);

        assert_eq!(
            entity.get("content"),
            Some(&Value::String("[1, 2, 3]".to_string()))
        );
        let Some(Value::Object(props)) = entity.get("properties") else {
            panic!(
                "properties must be parsed to an Object, got {:?}",
                entity.get("properties")
            );
        };
        assert_eq!(props.get("k"), Some(&Value::String("v".to_string())));
    }
}

#[cfg(test)]
mod cdc_coalescer_tests {
    use super::*;

    fn make_insert(view: &str, id: &str, value: &str) -> RowChange {
        let mut data = StorageEntity::new();
        data.insert("id".into(), Value::String(id.to_string()));
        data.insert("value".into(), Value::String(value.to_string()));
        data.insert("_rowid".into(), Value::String(id.to_string()));
        RowChange {
            relation_name: view.to_string(),
            change: ChangeData::Created {
                data,
                origin: ChangeOrigin::Remote {
                    operation_id: None,
                    trace_id: None,
                },
            },
        }
    }

    fn make_delete(view: &str, id: &str) -> RowChange {
        RowChange {
            relation_name: view.to_string(),
            change: ChangeData::Deleted {
                id: id.to_string(),
                origin: ChangeOrigin::Remote {
                    operation_id: None,
                    trace_id: None,
                },
            },
        }
    }

    fn make_update(view: &str, id: &str, value: &str) -> RowChange {
        let mut data = StorageEntity::new();
        data.insert("id".into(), Value::String(id.to_string()));
        data.insert("value".into(), Value::String(value.to_string()));
        data.insert("_rowid".into(), Value::String(id.to_string()));
        RowChange {
            relation_name: view.to_string(),
            change: ChangeData::Updated {
                id: id.to_string(),
                data,
                origin: ChangeOrigin::Remote {
                    operation_id: None,
                    trace_id: None,
                },
            },
        }
    }

    #[test]
    fn test_coalesce_delete_insert_becomes_update() {
        let result = coalesce_row_changes(vec![
            make_delete("view1", "id1"),
            make_insert("view1", "id1", "new_value"),
        ]);
        assert_eq!(result.len(), 1);
        match &result[0].change {
            ChangeData::Updated { id, data, .. } => {
                assert_eq!(id, "id1");
                assert_eq!(
                    data.get("value").unwrap(),
                    &Value::String("new_value".to_string())
                );
            }
            _ => panic!("Expected Update, got {:?}", result[0].change),
        }
    }

    #[test]
    fn test_coalesce_standalone_delete_unchanged() {
        let result = coalesce_row_changes(vec![make_delete("view1", "id1")]);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0].change, ChangeData::Deleted { .. }));
    }

    #[test]
    fn test_coalesce_standalone_insert_unchanged() {
        let result = coalesce_row_changes(vec![make_insert("view1", "id1", "value1")]);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0].change, ChangeData::Created { .. }));
    }

    #[test]
    fn test_coalesce_update_unchanged() {
        let result = coalesce_row_changes(vec![make_update("view1", "id1", "value1")]);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0].change, ChangeData::Updated { .. }));
    }

    #[test]
    fn test_coalesce_multiple_different_ids() {
        let result = coalesce_row_changes(vec![
            make_delete("view1", "id1"),
            make_insert("view1", "id1", "new1"),
            make_delete("view1", "id2"),
            make_insert("view1", "id2", "new2"),
        ]);
        assert_eq!(result.len(), 2);
        for change in &result {
            assert!(matches!(change.change, ChangeData::Updated { .. }));
        }
    }

    #[test]
    fn test_coalesce_different_views_not_coalesced() {
        let result = coalesce_row_changes(vec![
            make_delete("view1", "id1"),
            make_insert("view2", "id1", "value1"),
        ]);
        assert_eq!(result.len(), 2);
        assert!(matches!(result[0].change, ChangeData::Deleted { .. }));
        assert!(matches!(result[1].change, ChangeData::Created { .. }));
    }

    #[test]
    fn test_coalesce_insert_delete_different_id() {
        let result = coalesce_row_changes(vec![
            make_delete("view1", "id1"),
            make_insert("view1", "id2", "value"),
        ]);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_coalesce_insert_delete_becomes_noop() {
        let result = coalesce_row_changes(vec![
            make_insert("view1", "id1", "value1"),
            make_delete("view1", "id1"),
        ]);
        assert_eq!(result.len(), 0, "INSERT then DELETE should result in no-op");
    }

    #[test]
    fn test_coalesce_insert_delete_insert_becomes_update() {
        let result = coalesce_row_changes(vec![
            make_insert("view1", "id1", "value1"),
            make_delete("view1", "id1"),
            make_insert("view1", "id1", "value2"),
        ]);
        assert_eq!(result.len(), 1);
        match &result[0].change {
            ChangeData::Created { data, .. } => {
                assert_eq!(
                    data.get("value").unwrap(),
                    &Value::String("value2".to_string())
                );
            }
            _ => panic!("Expected Created, got {:?}", result[0].change),
        }
    }

    #[test]
    fn coalesce_handles_split_block_shape() {
        // The actual production shape from a recursive matview UPDATE,
        // post-Turso-fix (290fbb4ff) — surfaces as DELETE-then-INSERT,
        // which `coalesce_row_changes` folds to `Updated` directly.
        let raw = vec![
            make_insert("view1", "child_1_split", "two three"), // new sibling
            make_delete("view1", "child_1"),                    // old      -
            make_insert("view1", "child_1", "one"),             // truncated +
        ];
        let final_changes = coalesce_row_changes(raw);
        assert_eq!(final_changes.len(), 2);
        let kinds: Vec<&'static str> = final_changes
            .iter()
            .map(|c| match &c.change {
                ChangeData::Created { .. } => "Created",
                ChangeData::Updated { .. } => "Updated",
                ChangeData::Deleted { .. } => "Deleted",
                ChangeData::FieldsChanged { .. } => "FieldsChanged",
            })
            .collect();
        assert!(kinds.contains(&"Created"), "expected new sibling Insert");
        assert!(kinds.contains(&"Updated"), "expected truncated row Update");
    }
}

/// Integration tests that require a real database
/// These tests verify the backend's core functionality:
/// - Serialization of concurrent operations
/// - DDL works in all phases
/// - CDC subscriptions work correctly
#[cfg(test)]
mod integration_tests {
    use std::sync::Arc;

    use tempfile::tempdir;
    use tokio::sync::RwLock;

    use super::*;

    /// Helper to create a test backend
    async fn create_test_backend() -> Result<(Arc<RwLock<TursoBackend>>, DbHandle)> {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test_actor.db");

        // Open database
        let db = TursoBackend::open_database(&db_path)?;

        // Create CDC broadcast channel
        let (cdc_tx, _) = broadcast::channel(1024);

        // Create backend (which internally spawns the actor)
        let (backend, handle) = TursoBackend::new(db, cdc_tx)?;

        // Keep the temp dir alive
        std::mem::forget(temp_dir);

        Ok((Arc::new(RwLock::new(backend)), handle))
    }

    /// The live defect, end to end: the same filter written as a `:name`
    /// parameter and as an inline literal must select the same rows. Before the
    /// fix the parameterised form returned zero rows and no error, because
    /// `:pid` reached SQLite unbound and compared as NULL.
    #[tokio::test]
    async fn colon_named_parameter_selects_the_same_rows_as_the_literal() {
        let (_backend, handle) = create_test_backend().await.unwrap();
        handle
            .execute_ddl("CREATE TABLE b (id TEXT PRIMARY KEY, parent_id TEXT)")
            .await
            .unwrap();

        const PARENT: &str = "block:1820f890-aaaa-bbbb-cccc-ddddeeeeffff";
        for i in 0..5 {
            handle
                .execute(
                    "INSERT INTO b (id, parent_id) VALUES (?, ?)",
                    vec![
                        turso::Value::Text(format!("block:child-{i}")),
                        turso::Value::Text(PARENT.to_string()),
                    ],
                )
                .await
                .unwrap();
        }
        handle
            .execute(
                "INSERT INTO b (id, parent_id) VALUES (?, ?)",
                vec![
                    turso::Value::Text("block:elsewhere".to_string()),
                    turso::Value::Text("block:other-parent".to_string()),
                ],
            )
            .await
            .unwrap();

        let literal = handle
            .query(
                &format!("SELECT * FROM b WHERE parent_id = '{PARENT}'"),
                HashMap::new(),
            )
            .await
            .expect("literal query");
        assert_eq!(literal.len(), 5, "fixture: the literal form selects 5 rows");

        let mut params = HashMap::new();
        params.insert("pid".to_string(), Value::String(PARENT.to_string()));
        let bound = handle
            .query("SELECT * FROM b WHERE parent_id = :pid", params)
            .await
            .expect("named-parameter query");
        assert_eq!(
            bound.len(),
            literal.len(),
            "`:pid` must select what the literal selects, not silently nothing"
        );

        handle.shutdown().await.unwrap();
    }

    /// Test that DDL operations work and are properly serialized
    #[tokio::test]
    async fn test_ddl_operations() {
        let (_backend, handle) = create_test_backend().await.unwrap();

        // Create a table
        handle
            .execute_ddl("CREATE TABLE test_ddl (id TEXT PRIMARY KEY, value TEXT)")
            .await
            .expect("DDL should succeed");

        // Create an index
        handle
            .execute_ddl("CREATE INDEX idx_test_ddl_value ON test_ddl(value)")
            .await
            .expect("DDL for index should succeed");

        // Verify table exists by inserting data
        let insert_result = handle
            .execute(
                "INSERT INTO test_ddl (id, value) VALUES (?, ?)",
                vec![
                    turso::Value::Text("id1".to_string()),
                    turso::Value::Text("value1".to_string()),
                ],
            )
            .await;
        assert!(insert_result.is_ok(), "Insert after DDL should succeed");

        // Shutdown
        handle.shutdown().await.unwrap();
    }

    /// Test that DDL is allowed in Ready phase (for dynamic MatView creation)
    #[tokio::test]
    async fn test_ddl_allowed_in_ready_phase() {
        let (_backend, handle) = create_test_backend().await.unwrap();

        // Create initial table
        handle
            .execute_ddl("CREATE TABLE test_ready (id TEXT PRIMARY KEY, value INTEGER)")
            .await
            .unwrap();

        // Transition to Ready phase
        handle.transition_to_ready().await.unwrap();

        // Verify we're in Ready phase
        let phase = handle.get_phase().await.unwrap();
        assert_eq!(phase, DatabasePhase::Ready);

        // DDL should STILL work in Ready phase (for dynamic MatView creation)
        let ddl_result = handle
            .execute_ddl("CREATE TABLE another_table (id TEXT PRIMARY KEY)")
            .await;
        assert!(ddl_result.is_ok(), "DDL should work in Ready phase");

        // Create a view in Ready phase (simulates PRQL block MatView creation)
        let view_result = handle
            .execute_ddl("CREATE VIEW test_view AS SELECT * FROM test_ready WHERE value > 0")
            .await;
        assert!(
            view_result.is_ok(),
            "View creation should work in Ready phase"
        );

        handle.shutdown().await.unwrap();
    }

    /// Test that concurrent queries are serialized (no "database locked"
    /// errors)
    #[tokio::test]
    async fn test_query_serialization() {
        let (_backend, handle) = create_test_backend().await.unwrap();

        // Create table
        handle
            .execute_ddl("CREATE TABLE test_serial (id INTEGER PRIMARY KEY, value INTEGER)")
            .await
            .unwrap();

        // Insert some data
        for i in 0..10 {
            handle
                .execute(
                    "INSERT INTO test_serial (id, value) VALUES (?, ?)",
                    vec![turso::Value::Integer(i), turso::Value::Integer(i * 10)],
                )
                .await
                .unwrap();
        }

        handle.transition_to_ready().await.unwrap();

        // Spawn 100 concurrent queries
        let mut query_handles = Vec::new();
        for _ in 0..100 {
            let h = handle.clone();
            query_handles.push(tokio::spawn(async move {
                h.query("SELECT * FROM test_serial", HashMap::new()).await
            }));
        }

        // All queries should succeed (serialized by actor, no "database locked")
        let mut success_count = 0;
        for query_handle in query_handles {
            match query_handle.await {
                Ok(Ok(results)) => {
                    assert_eq!(results.len(), 10, "Each query should return 10 rows");
                    success_count += 1;
                }
                Ok(Err(e)) => {
                    panic!("Query failed with error: {:?}", e);
                }
                Err(e) => {
                    panic!("Task panicked: {:?}", e);
                }
            }
        }
        assert_eq!(
            success_count, 100,
            "All 100 concurrent queries should succeed"
        );

        handle.shutdown().await.unwrap();
    }

    /// Query path: user TEXT that merely looks like JSON must round-trip as
    /// a String (matching the CDC path), while the known JSON columns
    /// (`properties`; `data` via the UNION rewriter) come back structured.
    #[tokio::test]
    async fn test_json_shaped_text_round_trips_as_string() {
        let (_backend, handle) = create_test_backend().await.unwrap();

        handle
            .execute_ddl(
                "CREATE TABLE sniff_test (id TEXT PRIMARY KEY, content TEXT, properties TEXT)",
            )
            .await
            .unwrap();
        handle
            .execute(
                "INSERT INTO sniff_test (id, content, properties) VALUES (?, ?, ?)",
                vec![
                    turso::Value::Text("b1".into()),
                    turso::Value::Text("[1, 2, 3]".into()),
                    turso::Value::Text(r#"{"k": "v"}"#.into()),
                ],
            )
            .await
            .unwrap();
        handle.transition_to_ready().await.unwrap();

        // Named-params path (handle_query)
        let rows = handle
            .query("SELECT * FROM sniff_test", HashMap::new())
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].get("content"),
            Some(&Value::String("[1, 2, 3]".to_string())),
            "JSON-shaped user text must stay a String on the query path"
        );
        let Some(Value::Object(props)) = rows[0].get("properties") else {
            panic!(
                "properties must be parsed to an Object on the query path, got {:?}",
                rows[0].get("properties")
            );
        };
        assert_eq!(props.get("k"), Some(&Value::String("v".to_string())));

        // Positional-params path (handle_query_positional)
        let rows = handle
            .query_positional(
                "SELECT * FROM sniff_test WHERE id = ?",
                vec![turso::Value::Text("b1".into())],
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].get("content"),
            Some(&Value::String("[1, 2, 3]".to_string()))
        );
        assert!(matches!(rows[0].get("properties"), Some(Value::Object(_))));

        // json_group_array projection columns stay JSON TEXT (their consumers
        // parse strictly at their own boundary) — same as the CDC path.
        let rows = handle
            .query(
                "SELECT id, COALESCE(json_group_array(content) FILTER (WHERE content IS NOT \
                 NULL), '[]') AS tags FROM sniff_test GROUP BY id",
                HashMap::new(),
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].get("tags"),
            Some(&Value::String(r#"["[1, 2, 3]"]"#.to_string())),
            "aggregate JSON columns come back as JSON TEXT, not sniffed into Arrays"
        );

        // The UNION rewriter's synthesized `data` column still flattens.
        let rows = handle
            .query(
                r#"SELECT json_object('id', id, 'flat', 7) AS data FROM sniff_test"#,
                HashMap::new(),
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get("flat"), Some(&Value::Integer(7)));
        assert_eq!(rows[0].get("id"), Some(&Value::String("b1".to_string())));

        handle.shutdown().await.unwrap();
    }

    /// Test that interleaved DDL and DML operations are serialized correctly
    #[tokio::test]
    async fn test_ddl_dml_interleaved() {
        let (_backend, handle) = create_test_backend().await.unwrap();

        // Create initial table
        handle
            .execute_ddl("CREATE TABLE test_interleave (id TEXT PRIMARY KEY, value TEXT)")
            .await
            .unwrap();

        handle.transition_to_ready().await.unwrap();

        // Spawn interleaved DDL and DML operations
        let mut dml_handles = Vec::new();
        let mut ddl_handles = Vec::new();

        // DML operations (inserts)
        for i in 0..20 {
            let h = handle.clone();
            dml_handles.push(tokio::spawn(async move {
                h.execute(
                    "INSERT INTO test_interleave (id, value) VALUES (?, ?)",
                    vec![
                        turso::Value::Text(format!("id_{}", i)),
                        turso::Value::Text(format!("value_{}", i)),
                    ],
                )
                .await
            }));
        }

        // DDL operations (create views) - simulates PRQL block navigation
        for i in 0..5 {
            let h = handle.clone();
            ddl_handles.push(tokio::spawn(async move {
                h.execute_ddl(&format!(
                    "CREATE VIEW IF NOT EXISTS view_{} AS SELECT * FROM test_interleave WHERE id \
                     LIKE 'id_%'",
                    i
                ))
                .await
            }));
        }

        // All operations should succeed without "Database schema changed" errors
        let mut errors = Vec::new();
        for join_handle in dml_handles {
            match join_handle.await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => errors.push(format!("{:?}", e)),
                Err(e) => errors.push(format!("Task panicked: {:?}", e)),
            }
        }
        for join_handle in ddl_handles {
            match join_handle.await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => errors.push(format!("{:?}", e)),
                Err(e) => errors.push(format!("Task panicked: {:?}", e)),
            }
        }

        assert!(
            errors.is_empty(),
            "No errors expected from interleaved DDL/DML, got: {:?}",
            errors
        );

        handle.shutdown().await.unwrap();
    }

    /// Test phase transitions
    #[tokio::test]
    async fn test_phase_transitions() {
        let (_backend, handle) = create_test_backend().await.unwrap();

        // Initially in SchemaInit phase
        let phase = handle.get_phase().await.unwrap();
        assert_eq!(phase, DatabasePhase::SchemaInit);

        // Transition to Ready
        handle.transition_to_ready().await.unwrap();
        let phase = handle.get_phase().await.unwrap();
        assert_eq!(phase, DatabasePhase::Ready);

        // Shutdown transitions to ShuttingDown (implicitly during shutdown)
        handle.shutdown().await.unwrap();
    }

    /// Test transaction support
    #[tokio::test]
    async fn test_transactions() {
        let (_backend, handle) = create_test_backend().await.unwrap();

        // Create table
        handle
            .execute_ddl("CREATE TABLE test_tx (id INTEGER PRIMARY KEY, value TEXT)")
            .await
            .unwrap();

        // Execute multiple statements in a transaction
        let statements = vec![
            (
                "INSERT INTO test_tx (id, value) VALUES (1, 'first')".to_string(),
                vec![],
            ),
            (
                "INSERT INTO test_tx (id, value) VALUES (2, 'second')".to_string(),
                vec![],
            ),
            (
                "UPDATE test_tx SET value = 'updated' WHERE id = 1".to_string(),
                vec![],
            ),
        ];

        handle.transaction(statements).await.unwrap();

        // Verify transaction results
        let results = handle
            .query("SELECT * FROM test_tx ORDER BY id", HashMap::new())
            .await
            .unwrap();
        assert_eq!(results.len(), 2, "Should have 2 rows after transaction");

        handle.shutdown().await.unwrap();
    }
}
