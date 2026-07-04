//! The shipped chat-view SQL must MATCH its message rows, and must STREAM.
//!
//! Two independent defects made the chat view render zero bubbles, and the
//! shipped SQL is the seat of both:
//!
//!   - It compared `cc_message.session_id` against `$context_id`. A connector
//!     mirror stores the entity's own key scheme-qualified
//!     (`cc-session:<uuid>`) and every other column verbatim, so the foreign
//!     key holds the RAW id and the join matched nothing.
//!   - It named `cc_message_fdw`. `prime_fdw_caches` keys on the CACHE table,
//!     so naming the foreign table directly leaves the cache unprimed AND puts
//!     the matview on a foreign source, which emits no deltas — a one-shot
//!     snapshot instead of a stream
//!     (docs/Plans/turso-fdw-ivm-handoff-2026-08-04.md).
//!
//! `chat_view_render.rs` can see neither: it stubs `watch_query` with an
//! empty-but-live stream, so the query text is asserted but never run. Here the
//! shipped SQL is executed and watched through the production paths.
//!
//! @pbt kind harness
//! @pbt covers chat-view-message-join — the substituted chat-view SQL selects
//! the messages of the context row and streams later arrivals

#![cfg(feature = "pbt")]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_api::EntityUri;
use holon_api::QueryContext;
use holon_api::QueryLanguage;
use holon_api::Value;
use holon_api::render_types::Arg;
use holon_api::render_types::RenderExpr;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::reactive::table_expr;
use holon_integration_tests::TestEnvironment;

/// The `sql` a profile's `live_query` ships, read out of the sidecar rather
/// than restated, so a drifting profile cannot pass a stale hand-copy.
fn shipped_chat_sql(entity: &str) -> String {
    let render = shipped_render(entity);
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let expr = holon_api::render_dsl::parse_render_dsl(&render)
        .unwrap_or_else(|e| panic!("`{entity}` profile must parse: {e}"));
    find_live_query_sql(&expr)
        .unwrap_or_else(|| panic!("`{entity}` profile has no live_query with a literal `sql`"))
}

fn sidecar() -> serde_yaml::Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/integrations/claude-history.yaml"
    );
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read claude-history.yaml at {path}: {e}"));
    serde_yaml::from_str(&raw).unwrap_or_else(|e| panic!("parse claude-history.yaml: {e}"))
}

fn shipped_render(entity: &str) -> String {
    sidecar()["entities"][entity]["profile_variants"][0]["render"]
        .as_str()
        .unwrap_or_else(|| panic!("entity `{entity}` has no profile_variants[0].render"))
        .to_string()
}

fn find_live_query_sql(expr: &RenderExpr) -> Option<String> {
    match expr {
        RenderExpr::FunctionCall { name, args } => {
            if name == "live_query" {
                if let Some(sql) = args.iter().find_map(sql_arg) {
                    return Some(sql);
                }
            }
            args.iter().find_map(|a| find_live_query_sql(&a.value))
        }
        RenderExpr::Object { fields } => fields.values().find_map(find_live_query_sql),
        RenderExpr::Array { items } => items.iter().find_map(find_live_query_sql),
        _ => None,
    }
}

/// The DSL admits both `live_query(#{sql: "…"})` as one object argument and the
/// flattened `sql:` named argument, and which one the parser produces is its
/// business, not this test's.
fn sql_arg(arg: &Arg) -> Option<String> {
    if arg.name.as_deref() == Some("sql") {
        if let RenderExpr::Literal {
            value: Value::String(s),
        } = &arg.value
        {
            return Some(s.clone());
        }
    }
    match &arg.value {
        RenderExpr::Object { fields } => match fields.get("sql") {
            Some(RenderExpr::Literal {
                value: Value::String(s),
            }) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn context_for(row_id: &str) -> QueryContext {
    QueryContext::for_block(
        &EntityUri::parse(row_id).expect("a mirror row id is an EntityUri"),
        None,
    )
}

/// Run the shipped SQL exactly as a live_query does: bound against a
/// `QueryContext` whose current block is the mirror row's schemed key.
async fn rows_for(env: &TestEnvironment, sql: &str, row_id: &str) -> Vec<String> {
    let rows = env
        .engine()
        .execute_query(sql.to_string(), HashMap::new(), Some(context_for(row_id)))
        .await
        .unwrap_or_else(|e| panic!("chat-view SQL must execute: {e}\n{sql}"));
    rows.iter()
        .map(|r| match r.get("content") {
            Some(Value::String(s)) => s.clone(),
            other => panic!("every message row must carry text content, got {other:?}"),
        })
        .collect()
}

/// The mirror's shape: the primary key is scheme-qualified, every other column
/// is the provider's value verbatim — so foreign keys are RAW ids.
const SEED: &[&str] = &[
    "CREATE TABLE cc_message (uuid TEXT PRIMARY KEY, session_id TEXT, role TEXT, content TEXT, timestamp TEXT)",
    "CREATE TABLE cc_agent_message (uuid TEXT PRIMARY KEY, agent_id TEXT, role TEXT, content TEXT, timestamp TEXT)",
    "CREATE TABLE cc_live_session (id TEXT PRIMARY KEY, session_id TEXT, job_id TEXT)",
    "INSERT INTO cc_message VALUES ('m-1', 'sess-1', 'user', 'FIRST', '2026-08-03T10:00:00Z')",
    "INSERT INTO cc_message VALUES ('m-2', 'sess-1', 'assistant', 'SECOND', '2026-08-03T11:00:00Z')",
    // Excluded by the profile's own predicates, not by the join.
    "INSERT INTO cc_message VALUES ('m-3', 'sess-1', NULL, 'NO-ROLE', '2026-08-03T12:00:00Z')",
    "INSERT INTO cc_message VALUES ('m-4', 'sess-2', 'user', 'OTHER-SESSION', '2026-08-03T13:00:00Z')",
    "INSERT INTO cc_agent_message VALUES ('a-1', 'agent-1', 'user', 'AGENT-FIRST', '2026-08-03T10:00:00Z')",
    "INSERT INTO cc_agent_message VALUES ('a-2', 'agent-2', 'user', 'OTHER-AGENT', '2026-08-03T11:00:00Z')",
    "INSERT INTO cc_live_session VALUES ('cc-live-session:job-77', 'sess-1', 'job-77')",
];

/// Every entity whose profile carries a chat view. Named explicitly: deriving
/// the list by "entities whose render happens to yield a sql" makes a broken
/// extractor look like a clean sweep.
const CHAT_ENTITIES: &[&str] = &["session", "agent", "live_session"];

/// A matview over a foreign table is a snapshot: rows appearing at the source
/// stay invisible until an explicit REFRESH. No `live_query` may name one.
#[test]
fn no_live_query_reads_a_foreign_table_directly() {
    for entity in CHAT_ENTITIES {
        let sql = shipped_chat_sql(entity);
        assert!(
            !sql.contains("_fdw"),
            "`{entity}` watches a foreign table, which can only ever snapshot; watch the cache \
             table so write-through feeds the IVM circuit. sql:\n{sql}"
        );
    }
}

#[test]
fn chat_view_sql_selects_the_messages_of_its_context_row() {
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap(),
    );
    runtime.clone().block_on(run(runtime.clone()));
}

async fn run(runtime: Arc<tokio::runtime::Runtime>) {
    let env = TestEnvironment::new(runtime).expect("new TestEnvironment");
    env.start_app(false).await.expect("start_app");
    let db = env.engine().db_handle().clone();
    db.transition_to_ready()
        .await
        .expect("transition the actor to Ready");
    for stmt in SEED {
        db.execute_ddl(stmt)
            .await
            .unwrap_or_else(|e| panic!("seed statement must succeed: {e}\n{stmt}"));
    }

    // A transcript view: `cc_message.session_id` holds `sess-1` while the row
    // key is `cc-session:sess-1`.
    let session_sql = shipped_chat_sql("session");
    assert_eq!(
        rows_for(&env, &session_sql, "cc-session:sess-1").await,
        vec!["FIRST".to_string(), "SECOND".to_string()],
        "the session chat view must select that session's messages in order; sql:\n{session_sql}"
    );

    // The agent view has the identical shape on `agent_id`.
    let agent_sql = shipped_chat_sql("agent");
    assert_eq!(
        rows_for(&env, &agent_sql, "cc-agent:agent-1").await,
        vec!["AGENT-FIRST".to_string()],
        "the agent chat view must select that agent's messages; sql:\n{agent_sql}"
    );

    // The live view correlates through the mirror instead: its context row is a
    // `cc_live_session`, whose `session_id` column already holds the raw
    // transcript id.
    let live_sql = shipped_chat_sql("live_session");
    assert_eq!(
        rows_for(&env, &live_sql, "cc-live-session:job-77").await,
        vec!["FIRST".to_string(), "SECOND".to_string()],
        "the live-session chat view must select the correlated transcript's messages; \
         sql:\n{live_sql}"
    );

    stream_check(&env, &session_sql).await;
}

/// The property a snapshot cannot satisfy: a message written after the view is
/// open reaches the watcher with no REFRESH and no poll.
///
/// Scope, so this is not mistaken for the guard on the `_fdw` routing: there is
/// no FDW in this environment at all, so it demonstrates that a matview over
/// the cache table streams — it cannot fail for the reason the routing defect
/// existed. `no_live_query_reads_a_foreign_table_directly` is that guard.
async fn stream_check(env: &TestEnvironment, sql: &str) {
    let reactive: Arc<ReactiveEngine> = env
        .reactive_engine
        .get()
        .expect("start_app must resolve a ReactiveEngine")
        .clone();
    let services: Arc<dyn BuilderServices> = reactive.clone();

    let (key, live) = reactive.watch_query_live(
        sql.to_string(),
        QueryLanguage::HolonSql,
        table_expr(),
        Some(context_for("cc-session:sess-1")),
        services.clone(),
    );
    let rows = reactive.ensure_watching(&key);

    await_rows(
        &rows,
        2,
        "the chat view must open with the session's stored messages",
    )
    .await;

    let db = env.engine().db_handle().clone();
    db.execute_values(
        "INSERT INTO cc_message (uuid, session_id, role, content, timestamp) VALUES (?, ?, ?, ?, \
         ?)",
        vec![
            Value::String("m-5".to_string()),
            Value::String("sess-1".to_string()),
            Value::String("assistant".to_string()),
            Value::String("ARRIVED-LATE".to_string()),
            Value::String("2026-08-03T14:00:00Z".to_string()),
        ],
    )
    .await
    .expect("a later message lands in the cache table");

    await_rows(
        &rows,
        3,
        "a message arriving after the view opened must stream in — a matview over a foreign \
         table would still show the old snapshot",
    )
    .await;

    drop(live);
}

async fn await_rows(rows: &holon_frontend::reactive::ReactiveRenderedRows, want: usize, why: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (_expr, snapshot) = rows.snapshot();
        if snapshot.len() == want && rows.error().is_none() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{why}: rows={} want={want} error={:?}",
            snapshot.len(),
            rows.error()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
