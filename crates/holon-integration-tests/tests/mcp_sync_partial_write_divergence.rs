//! What the sync engine SAYS it applied must match what the table holds.
//!
//! One dogfood observation, two independent defects, one entry each.
//!
//! `2026-09-19-todoist-full-sync-mirror-divergence-kills-the-replica` — the
//! cache DECLINES rows and reports success. `QueryableCache` writes every
//! `Created` as `INSERT OR IGNORE`, and SQLite's `OR IGNORE` skips a row
//! violating ANY constraint; a sidecar column is `NOT NULL` unless the YAML
//! says otherwise, so a record missing one field is dropped while
//! `apply_batch` returns `Ok`. The engine then writes the full batch into its
//! mirror. That is why `todoist_tasks` read 0 rows.
//!
//! `2026-09-19-pagination-cursor-stored-as-sync-token-truncates-the-replica` —
//! a PAGE cursor was persisted as the entity's SYNC TOKEN and only one page
//! was fetched per sync, so the last page (which carries no cursor) reached
//! the full-sync diff as if it were the whole table and deleted everything
//! else. That is the `96 → 13` collapse, and it is not caused by, or fixed by,
//! the declined-rows defect above.
//!
//! These tests drive the REAL `McpSyncEngine` over a REAL `QueryableCache`
//! against an in-memory Turso database, with a scripted call surface standing
//! in for the provider. Nothing here is a double of the write path — the
//! behaviour that matters (a cache that declines a row) exists only in the
//! real SQL write path.
//!
//! @pbt kind harness
//! @pbt covers mcp-sync-partial-write — a batch the cache declines must fail
//! the sync loudly instead of diverging the mirror
//! @pbt covers mcp-sync-pagination — one sync fetches every page, and a page
//! cursor is never persisted as a sync token

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon::core::queryable_cache::QueryableCache;
use holon::storage::DbHandle;
use holon_api::DynamicEntity;
use holon_api::StreamPosition;
use holon_core::EntityCache;
use holon_core::SyncTokenStore;
use holon_core::SyncableProvider;
use holon_mcp_client::mcp_call_surface::McpCallSurface;
use holon_mcp_client::mcp_sidecar::McpSidecar;
use holon_mcp_client::mcp_sync_engine::McpSyncEngine;
use holon_turso::turso::TursoBackend;
use rmcp::model::CallToolRequestParam;
use rmcp::model::CallToolResult;
use rmcp::model::Content;
use rmcp::model::ReadResourceRequestParam;
use rmcp::model::ReadResourceResult;
use rmcp::service::ServiceError;

const PROVIDER: &str = "fixture";
/// A resumable incremental sync token — persisted across syncs.
const SYNC_TOKEN: &str =
    "\n      cursor:\n        request_param: cursor\n        response_field: nextCursor";
/// Paging within one fetch — followed to exhaustion, never persisted.
const PAGINATE: &str =
    "\n      paginate:\n        request_param: cursor\n        response_field: nextCursor";
const PROJECTS: &str = "fx_projects";
const TASKS: &str = "fx_tasks";

/// One entity whose every column is `NOT NULL` — the shape every shipped
/// sidecar has, since `FieldSchema::nullable` defaults to false.
fn projects_yaml(cursor_block: &str) -> String {
    format!(
        r#"
entities:
  {PROJECTS}:
    id_column: id
    schema:
      - {{ name: id, sql_type: TEXT, primary_key: true }}
      - {{ name: name, sql_type: TEXT }}
      - {{ name: parentId, sql_type: TEXT }}
    sync:
      list_tool: find-projects
      extract_path: projects{cursor_block}
tools: {{}}
"#
    )
}

/// Two entities, so a failure on one can be measured against the other.
fn two_entity_yaml() -> String {
    format!(
        r#"
entities:
  {PROJECTS}:
    id_column: id
    schema:
      - {{ name: id, sql_type: TEXT, primary_key: true }}
      - {{ name: name, sql_type: TEXT }}
      - {{ name: parentId, sql_type: TEXT }}
    sync:
      list_tool: find-projects
      extract_path: projects
  {TASKS}:
    id_column: id
    schema:
      - {{ name: id, sql_type: TEXT, primary_key: true }}
      - {{ name: content, sql_type: TEXT }}
    sync:
      list_tool: find-tasks
      extract_path: tasks
tools: {{}}
"#
    )
}

/// A call surface that answers each tool name with a canned JSON object, the
/// way the provider's list tool does.
#[derive(Debug)]
struct ScriptedSurface {
    responses: Mutex<HashMap<String, serde_json::Value>>,
}

impl ScriptedSurface {
    fn new(responses: &[(&str, serde_json::Value)]) -> Self {
        Self {
            responses: Mutex::new(
                responses
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), v.clone()))
                    .collect(),
            ),
        }
    }
}

#[async_trait]
impl McpCallSurface for ScriptedSurface {
    async fn call_tool(
        &self,
        params: CallToolRequestParam,
    ) -> Result<CallToolResult, ServiceError> {
        let body = self
            .responses
            .lock()
            .unwrap()
            .get(params.name.as_ref())
            .unwrap_or_else(|| panic!("the test scripted no response for tool '{}'", params.name))
            .to_string();
        Ok(CallToolResult::success(vec![Content::text(body)]))
    }

    async fn read_resource(
        &self,
        params: ReadResourceRequestParam,
    ) -> Result<ReadResourceResult, ServiceError> {
        panic!(
            "this fixture syncs by tool, never by resource ('{}')",
            params.uri
        )
    }
}

/// A call surface that pages: the response depends on the `cursor` argument
/// the caller passes back, the way a real paginating list tool behaves.
#[derive(Debug)]
struct PagingSurface {
    /// Keyed by the incoming cursor argument — `None` is the first page.
    pages: HashMap<Option<String>, serde_json::Value>,
    calls: Mutex<usize>,
}

impl PagingSurface {
    fn new(pages: Vec<(Option<&str>, serde_json::Value)>) -> Self {
        Self {
            pages: pages
                .into_iter()
                .map(|(k, v)| (k.map(str::to_string), v))
                .collect(),
            calls: Mutex::new(0),
        }
    }

    fn calls(&self) -> usize {
        *self.calls.lock().unwrap()
    }
}

#[async_trait]
impl McpCallSurface for PagingSurface {
    async fn call_tool(
        &self,
        params: CallToolRequestParam,
    ) -> Result<CallToolResult, ServiceError> {
        *self.calls.lock().unwrap() += 1;
        let cursor = params
            .arguments
            .as_ref()
            .and_then(|a| a.get("cursor"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let body = self
            .pages
            .get(&cursor)
            .unwrap_or_else(|| panic!("the test scripted no page for cursor {cursor:?}"))
            .to_string();
        Ok(CallToolResult::success(vec![Content::text(body)]))
    }

    async fn read_resource(
        &self,
        params: ReadResourceRequestParam,
    ) -> Result<ReadResourceResult, ServiceError> {
        panic!(
            "this fixture syncs by tool, never by resource ('{}')",
            params.uri
        )
    }
}

#[derive(Default)]
struct InMemoryTokenStore {
    tokens: Mutex<HashMap<String, StreamPosition>>,
}

impl InMemoryTokenStore {
    fn saved(&self, key: &str) -> Option<StreamPosition> {
        self.tokens.lock().unwrap().get(key).cloned()
    }
}

#[async_trait]
impl SyncTokenStore for InMemoryTokenStore {
    async fn load_token(&self, provider_name: &str) -> holon_core::Result<Option<StreamPosition>> {
        Ok(self.tokens.lock().unwrap().get(provider_name).cloned())
    }
    async fn save_token(
        &self,
        provider_name: &str,
        position: StreamPosition,
    ) -> holon_core::Result<()> {
        self.tokens
            .lock()
            .unwrap()
            .insert(provider_name.to_string(), position);
        Ok(())
    }
    async fn clear_all_tokens(&self) -> holon_core::Result<()> {
        self.tokens.lock().unwrap().clear();
        Ok(())
    }
}

/// The engine, its caches and the token store, all wired over one in-memory
/// Turso database.
struct Fixture {
    engine: Arc<McpSyncEngine>,
    caches: HashMap<String, Arc<dyn EntityCache<DynamicEntity>>>,
    tokens: Arc<InMemoryTokenStore>,
    db: DbHandle,
}

impl Fixture {
    async fn row_count(&self, entity: &str) -> usize {
        let table = self
            .engine
            .sidecar()
            .prefixed_name(entity)
            .table_name()
            .to_string();
        let rows = self
            .db
            .query(&format!("SELECT id FROM \"{table}\""), HashMap::new())
            .await
            .expect("count rows");
        rows.len()
    }

    async fn cache_ids(&self, entity: &str) -> usize {
        self.caches[entity]
            .get_all_ids()
            .await
            .expect("get_all_ids")
            .len()
    }
}

async fn fixture(yaml: &str, responses: &[(&str, serde_json::Value)]) -> Fixture {
    fixture_with_surface(yaml, Arc::new(ScriptedSurface::new(responses))).await
}

async fn fixture_with_surface(yaml: &str, surface: Arc<dyn McpCallSurface>) -> Fixture {
    let (backend, db) = TursoBackend::new_in_memory().await.expect("in-memory db");
    // The actor owns the connection for the whole test; dropping the backend
    // would close it out from under the engine.
    std::mem::forget(backend);

    let sidecar: McpSidecar = serde_yaml::from_str(yaml).expect("parse fixture sidecar");

    let mut caches: HashMap<String, Arc<dyn EntityCache<DynamicEntity>>> = HashMap::new();
    let mut strategies: HashMap<String, Box<dyn holon_mcp_client::SyncStrategy>> = HashMap::new();
    for (name, config) in &sidecar.entities {
        let entity = sidecar.prefixed_name(name);
        let type_def = config
            .to_type_definition(
                entity.table_name().as_str(),
                PROVIDER,
                sidecar.write_ownership(name),
            )
            .expect("fixture entity declares a schema");
        let cache = QueryableCache::<DynamicEntity>::new(db.clone(), type_def)
            .await
            .expect("create cache table");
        caches.insert(name.clone(), Arc::new(cache));
        strategies.insert(
            name.clone(),
            config
                .sync
                .as_ref()
                .expect("fixture entity declares a sync")
                .into_strategy()
                .expect("strategy"),
        );
    }

    let tokens = Arc::new(InMemoryTokenStore::default());
    let engine = Arc::new(McpSyncEngine::new(
        surface,
        None,
        strategies,
        caches.clone(),
        tokens.clone(),
        PROVIDER.to_string(),
        sidecar,
        Vec::new(),
        Some(db.clone()),
    ));

    Fixture {
        engine,
        caches,
        tokens,
        db,
    }
}

fn project(id: &str, name: &str, parent: Option<&str>) -> serde_json::Value {
    match parent {
        Some(p) => serde_json::json!({ "id": id, "name": name, "parentId": p }),
        // The field is ABSENT, which is what a provider sends for a top-level
        // record and what binds SQL NULL into a NOT NULL column.
        None => serde_json::json!({ "id": id, "name": name }),
    }
}

/// MEASUREMENT, not a regression pin. The dogfood hypothesis was that a page
/// carrying duplicate primary keys diverges the mirror from the cache. It does
/// not: the mirror keys rows through `LiveData`'s map and SQL keys them through
/// the primary key, so both collapse the duplicate to one row. Recorded here
/// so the hypothesis is not re-tried.
#[tokio::test]
async fn duplicate_primary_keys_in_one_page_do_not_diverge_the_mirror() {
    let fx = fixture(
        &projects_yaml(""),
        &[(
            "find-projects",
            serde_json::json!({ "projects": [
                project("p1", "One", Some("")),
                project("p1", "One", Some("")),
                project("p2", "Two", Some("")),
            ]}),
        )],
    )
    .await;

    fx.engine
        .sync(StreamPosition::Beginning)
        .await
        .expect("a page with a duplicate primary key is not by itself a divergence");

    assert_eq!(
        fx.row_count(PROJECTS).await,
        2,
        "three records with two distinct ids must land two rows"
    );
}

/// THE RED. One record omits `parentId`, the column the sidecar declares
/// `NOT NULL` by default. `INSERT OR IGNORE` skips that row, `apply_batch`
/// returns `Ok`, and the engine writes all three into its mirror.
#[tokio::test]
async fn a_full_sync_the_cache_partly_declined_fails_loudly() {
    let fx = fixture(
        &projects_yaml(""),
        &[(
            "find-projects",
            serde_json::json!({ "projects": [
                project("p1", "One", Some("")),
                project("p2", "Two", Some("")),
                project("p3", "Three", None),
            ]}),
        )],
    )
    .await;

    let outcome = fx.engine.sync(StreamPosition::Beginning).await;

    let landed = fx.row_count(PROJECTS).await;
    assert_eq!(
        landed, 2,
        "the fixture must actually exercise a declined row — got {landed} of 3 landed"
    );
    let err = outcome.expect_err(
        "a batch of 3 that landed 2 rows is a partial write: the sync must refuse it, not report \
         success and carry a mirror the table never agreed with",
    );
    let msg = format!("{err}");
    assert!(
        msg.contains(PROJECTS) && msg.contains("p3"),
        "the refusal must name the entity and the row the cache declined — got: {msg}"
    );
}

/// The other half of the same escape, on the cursor path: the first boot
/// logged `incremental sync applied records=10` with the table at zero, and
/// saved the cursor anyway, so those records were unreachable forever.
#[tokio::test]
async fn an_incremental_batch_the_cache_declined_fails_and_keeps_the_cursor() {
    let fx = fixture(
        &projects_yaml(SYNC_TOKEN),
        &[(
            "find-projects",
            serde_json::json!({
                "projects": [project("p1", "One", None), project("p2", "Two", None)],
                "nextCursor": "page-2",
            }),
        )],
    )
    .await;

    let outcome = fx.engine.sync(StreamPosition::Beginning).await;

    assert_eq!(
        fx.row_count(PROJECTS).await,
        0,
        "the fixture must actually exercise a fully declined incremental batch"
    );
    outcome.expect_err(
        "an incremental batch of 2 that landed 0 rows must fail the sync, not report records \
         applied",
    );
    assert!(
        fx.tokens.saved(&format!("{PROVIDER}.{PROJECTS}")).is_none(),
        "advancing the cursor past records that never landed makes them unreachable forever"
    );
}

/// Four complete records spread over two pages: page 1 carries three and a
/// `nextCursor`, page 2 carries the fourth and no cursor.
fn two_pages() -> Arc<PagingSurface> {
    Arc::new(PagingSurface::new(vec![
        (
            None,
            serde_json::json!({
                "projects": [
                    project("p1", "One", Some("")),
                    project("p2", "Two", Some("")),
                    project("p3", "Three", Some("")),
                ],
                "nextCursor": "page-2",
            }),
        ),
        (
            Some("page-2"),
            serde_json::json!({ "projects": [project("p4", "Four", Some(""))] }),
        ),
    ]))
}

/// One sync must fetch the WHOLE result set. A page cursor lives inside one
/// fetch; stopping after page 1 and persisting its cursor as the entity's sync
/// token makes a later sync resume mid-set.
///
/// Every record here carries every column, so the declined-rows mechanism is
/// provably not in play.
#[tokio::test]
async fn one_sync_fetches_every_page_of_a_paginating_provider() {
    let surface = two_pages();
    let fx = fixture_with_surface(&projects_yaml(PAGINATE), surface.clone()).await;

    fx.engine
        .sync(StreamPosition::Beginning)
        .await
        .expect("a complete two-page fetch is not an error");

    assert_eq!(
        surface.calls(),
        2,
        "the engine must follow nextCursor within one sync, not stop after page 1"
    );
    assert_eq!(
        fx.row_count(PROJECTS).await,
        4,
        "all four records exist at the provider, so one sync must land all four"
    );
    assert!(
        fx.tokens.saved(&format!("{PROVIDER}.{PROJECTS}")).is_none(),
        "a page cursor is valid only inside the fetch that produced it and must never be \
         persisted as the entity's sync token"
    );
}

/// THE PRODUCTION SHAPE. The last page carries no `nextCursor`, so it took the
/// full-sync branch and was diffed against the WHOLE table — deleting every
/// row not on that one page, with `sync()` returning Ok. That is the
/// `96 → 13` collapse, and it re-pins the table on every later sync.
#[tokio::test]
async fn a_second_sync_does_not_delete_the_rows_from_earlier_pages() {
    let surface = two_pages();
    let fx = fixture_with_surface(&projects_yaml(PAGINATE), surface.clone()).await;

    fx.engine
        .sync(StreamPosition::Beginning)
        .await
        .expect("first sync");
    let after_first = fx.row_count(PROJECTS).await;

    fx.engine
        .sync(StreamPosition::Beginning)
        .await
        .expect("second sync");
    let after_second = fx.row_count(PROJECTS).await;

    assert_eq!(
        (after_first, after_second),
        (4, 4),
        "re-syncing a provider that still has all four records must not shrink the replica"
    );
}

/// The refusal has to reach the user. A panic on the detached sync task left
/// the row at `Syncing` forever; an `Err` folds into the health signal the
/// app's status writer subscribes to, which
/// `frontend_suite/integration_state_sync_failing.rs` carries the rest of the
/// way to `Sync failing` in `integration_state`.
#[tokio::test]
async fn a_refused_initial_sync_marks_the_integration_unhealthy() {
    let fx = fixture(
        &projects_yaml(""),
        &[(
            "find-projects",
            serde_json::json!({ "projects": [project("p1", "One", None)] }),
        )],
    )
    .await;

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let loop_handle = holon_mcp_client::spawn_sync_event_loop(
        rx,
        fx.engine.clone(),
        holon_mcp_client::SyncGate::opened(),
        holon_mcp_client::SyncLoopTuning::test(),
    );
    tx.send(holon_mcp_client::SyncEvent::SyncAll)
        .expect("enqueue the initial sync");
    drop(tx);
    loop_handle.await.expect("sync loop");

    assert_eq!(
        fx.engine.health().get(),
        holon_mcp_client::SyncHealth::Failing,
        "an initial sync whose only row the cache declined must not leave the integration \
         looking like it is still syncing"
    );
}

/// One entity refusing its page must not starve the others: `todoist_tasks`
/// was never fetched at all because the projects entity aborted the sweep.
#[tokio::test]
async fn one_failing_entity_does_not_starve_the_rest_of_the_sweep() {
    let fx = fixture(
        &two_entity_yaml(),
        &[
            (
                "find-projects",
                serde_json::json!({ "projects": [project("p1", "One", None)] }),
            ),
            (
                "find-tasks",
                serde_json::json!({ "tasks": [
                    { "id": "t1", "content": "first" },
                    { "id": "t2", "content": "second" },
                ]}),
            ),
        ],
    )
    .await;

    let outcome = fx.engine.sync(StreamPosition::Beginning).await;

    outcome.expect_err("the projects entity declined its only row, so the sweep failed");
    assert_eq!(
        fx.row_count(TASKS).await,
        2,
        "the tasks entity is independent and its rows must land whatever projects did"
    );
    assert_eq!(
        fx.cache_ids(TASKS).await,
        2,
        "the tasks cache must agree with its table"
    );
}
