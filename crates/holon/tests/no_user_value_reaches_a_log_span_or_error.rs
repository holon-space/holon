//! Contract: a realistic `BackendEngine` workload puts no user value into any
//! tracing event, any span field, or any error string handed back to a caller.
//!
//! The earlier oracle for this contract drove `DbHandle` alone. `DbHandle` is
//! the LAST seam a statement crosses, so a redaction applied there proves
//! nothing about the layers above it: `MatviewManager` builds a
//! `CREATE MATERIALIZED VIEW … AS <statement>` and logs that, `preload` logs it
//! again on failure, and a dozen `with_context` / `panic!` sites interpolate
//! the statement into text that leaves the process as an error. Every one of
//! those carries the values, because `BackendEngine::inline_parameters` puts
//! the values INTO the statement text before any of them sees it.
//!
//! So this oracle drives the whole engine — a write, a watched query, a
//! failing view creation, a refused watch, a failing query — and watches every
//! target at TRACE at once, errors included. A value that appears anywhere in
//! that capture is a leak, whichever layer wrote it.
//!
//! Two dependencies are excluded by name. `turso_core` logs each statement AND
//! each bound string value of its program at DEBUG; `sqlparser` logs every
//! statement it parses plus each string literal it reads. Both are the
//! dependencies' own code, and the production filter
//! (`holon_gpui=info,holon=info,holon_tui=info`,
//! crates/holon-frontend/src/logging.rs) leaves both at the default ERROR, so
//! only an explicit `RUST_LOG` turns them on. Everything Holon itself writes
//! is in scope at TRACE.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Context as _;
use anyhow::Result;
use holon::api::backend_engine::BackendEngine;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_READ_TABLE;
use holon::storage::BLOCK_WRITE_TABLE;
use holon::testing::e2e_test_helpers::E2ETestContext;
use holon_api::Value;
use holon_api::block::Block;
use holon_core::OperationProvider;
use holon_core::storage::types::StorageEntity;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;
use tokio::runtime::Handle;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// User values the workload writes and then queries for. Each is distinctive
/// enough that a substring search cannot match surrounding SQL by accident.
const SENTINEL_CONTENT: &str = "ZqSentinelBlockBodyZq";
const SENTINEL_PROPERTY: &str = "ZqSentinelPropertyValueZq";
const SENTINEL_NUMBER: &str = "88150377";

const ROOT_PARENT: &str = "sentinel:no_parent";

/// Everything a reader of this process could see: the formatted trace output
/// plus every error string the workload received back.
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Seen {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("capture poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Seen {
    /// Fold an error the caller received into the same body of text: an error
    /// that carries a value off the machine does it through its `Display`, not
    /// through a log line.
    fn error(&self, what: &str, e: &anyhow::Error) {
        use std::io::Write as _;
        let mut sink = self.clone();
        writeln!(sink, "[error returned to caller] {what}: {e:#}").expect("in-memory write");
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("capture poisoned").clone()).into_owned()
    }
}

/// `FmtSpan::NEW` is where `#[tracing::instrument]` records its fields, which
/// is where a statement enters a span. `trace` for every target: the leak this
/// oracle exists for was in a crate the earlier, target-scoped capture did not
/// watch.
fn capture() -> Seen {
    let seen = Seen::default();
    let writer = seen.clone();
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(move || writer.clone())
                .with_ansi(false)
                .with_span_events(FmtSpan::NEW)
                .with_filter(EnvFilter::new("trace,turso_core=off,sqlparser=off")),
        )
        // Each nextest test owns its process, so this is the only subscriber
        // there.
        .try_init()
        .ok();
    seen
}

/// The production SqlOnly block wiring, as `watch_context_membership_leak.rs`:
/// `E2ETestContext::new()` boots an engine with no block provider, and a
/// workload that cannot write a block never puts a user value into a
/// statement.
async fn block_engine() -> Result<Arc<BackendEngine>> {
    create_test_engine_with_providers(":memory:".into(), |module| {
        module
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle,
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    BlockSchemaModule.edge_fields(),
                )) as Arc<dyn OperationProvider>
            })
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                let sql_ops = Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle.clone(),
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    BlockSchemaModule.edge_fields(),
                ));
                let mut block_raw_type_def = Block::type_definition();
                block_raw_type_def.name = BLOCK_WRITE_TABLE.to_string();
                let cache = tokio::task::block_in_place(|| {
                    let handle = Handle::current();
                    // ALLOW(block_on): wrapped in `block_in_place`, which is what makes a
                    // blocking wait legal on a multi-thread runtime thread.
                    handle.block_on(QueryableCache::<Block>::new(db_handle, block_raw_type_def))
                })
                .expect("block_raw cache");
                Arc::new(SqlBlockOperations::new(sql_ops, Arc::new(cache)))
                    as Arc<dyn OperationProvider>
            })
    })
    .await
    .context("this oracle's engine must boot with the block provider")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_user_value_reaches_a_log_span_or_error() -> Result<()> {
    let seen = capture();
    let ctx = E2ETestContext::from_engine(block_engine().await?);
    let engine = ctx.service().engine().clone();

    // 1. A write carrying a user value in content AND in a property, with a number
    //    among the property values.
    let mut params = StorageEntity::new();
    params.insert("id".into(), Value::String("block:sentinel-1".into()));
    params.insert("content".into(), Value::String(SENTINEL_CONTENT.into()));
    params.insert("parent_id".into(), Value::String(ROOT_PARENT.into()));
    params.insert(
        "properties".into(),
        Value::Object(HashMap::from([
            (
                "sentinel-note".to_string(),
                Value::String(SENTINEL_PROPERTY.into()),
            ),
            (
                "sentinel-amount".to_string(),
                Value::Integer(SENTINEL_NUMBER.parse().expect("the sentinel is a number")),
            ),
        ])),
    );
    ctx.execute_op("block", "create", params)
        .await
        .context("the write under test must really run")?;

    // 2. A watched query whose predicate holds the values: it goes through
    //    `inline_parameters`, `ensure_view`'s CREATE and the DDL path.
    let watched = format!(
        "SELECT b.id, b.content FROM {BLOCK_READ_TABLE} b WHERE b.content = '{SENTINEL_CONTENT}' \
         AND length(b.content) < {SENTINEL_NUMBER}"
    );
    let stream = engine
        .query_and_watch(watched, HashMap::new(), None)
        .await
        .context("the watched query under test must really run")?;
    drop(stream);

    // 3. A view creation that FAILS — `preload` discloses the failure instead of
    //    propagating it, and that disclosure carried the whole CREATE.
    let unbuildable = format!(
        "SELECT x FROM a_table_that_does_not_exist WHERE x = '{SENTINEL_PROPERTY}' AND y = \
         {SENTINEL_NUMBER}"
    );
    engine
        .preload_views(&[unbuildable.as_str()])
        .await
        .context("a preload failure is disclosed, not propagated")?;

    // 4. A watch REFUSED by holon itself rather than by the engine: the clock
    //    relation with no grain literal. It is the error path that reads the
    //    statement back out to explain the refusal.
    let clock_without_grain = format!(
        "SELECT grain, epoch_day FROM clock WHERE grain = '{SENTINEL_PROPERTY}' AND epoch_day < \
         {SENTINEL_NUMBER}"
    );
    match engine
        .query_and_watch(clock_without_grain, HashMap::new(), None)
        .await
    {
        Ok(_) => panic!("a clock watch that names no grain must be refused"),
        Err(e) => seen.error("query_and_watch over clock with no grain", &e),
    }

    // 5. A query that fails at execution, carrying the values in its text.
    let failing = format!(
        "SELECT no_such_column FROM {BLOCK_READ_TABLE} WHERE content = '{SENTINEL_CONTENT}' AND \
         rowid = {SENTINEL_NUMBER}"
    );
    match engine.execute_query(failing, HashMap::new(), None).await {
        Ok(_) => panic!("a statement naming no such column must not succeed"),
        Err(e) => seen.error("execute_query over a missing column", &e),
    }

    let text = seen.text();
    assert!(
        text.len() > 2000,
        "the capture holds {} bytes, too few for this workload to have been observed at all",
        text.len()
    );

    let leaks: Vec<String> = [
        ("block content", SENTINEL_CONTENT),
        ("a property value", SENTINEL_PROPERTY),
        ("a numeric property value", SENTINEL_NUMBER),
    ]
    .into_iter()
    .filter(|(_, sentinel)| text.contains(sentinel))
    .map(|(name, sentinel)| format!("{name} ({sentinel}):\n{}", lines_with(&text, sentinel)))
    .collect();
    assert!(
        leaks.is_empty(),
        "a user value reached a log line, a span field or an error:\n\n{}",
        leaks.join("\n\n")
    );

    // The other half of the contract: a redaction that blanked the statement
    // CODE as well would pass the loop above and leave every span and every
    // error message useless.
    for shape in [
        "CREATE MATERIALIZED VIEW",
        BLOCK_READ_TABLE,
        // The refusal that reads the statement back to explain itself: without
        // this line the capture holds no error built from a statement at all,
        // and the loop above would be vacuous for that whole class.
        "names no grain as a literal",
        // Only step 3's failing preload (the matview_manager Err arm) writes this.
        "preload: failed to create view",
        // Only step 5's failing query is refused at statement preparation.
        "Failed to prepare query",
    ] {
        assert!(
            text.contains(shape),
            "redaction erased {shape:?} — the statement shape must survive, or the spans and \
             errors say nothing"
        );
    }

    Ok(())
}

fn lines_with(text: &str, needle: &str) -> String {
    text.lines()
        .filter(|l| l.contains(needle))
        .take(40)
        .collect::<Vec<_>>()
        .join("\n")
}
