//! A database whose `block_raw` was created before `block_type` became
//! nullable and `completed` was dropped keeps that shape across boots
//! (`CREATE TABLE IF NOT EXISTS`). Reads, creates and `block_type` writes must
//! still work on it.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon::api::holon_service::HolonService;
use holon_api::Block;
use holon_api::EntityName;
use holon_api::Value;
use holon_integration_tests::test_environment::TestEnvironment;

const LEGACY_BLOCK_RAW: &str = "CREATE TABLE block_raw (
    id TEXT PRIMARY KEY,
    parent_id TEXT,
    sort_key TEXT NOT NULL DEFAULT 'A0',
    content TEXT NOT NULL DEFAULT '',
    content_type TEXT NOT NULL DEFAULT 'text',
    source_language TEXT,
    source_name TEXT,
    properties TEXT,
    property_kinds TEXT,
    marks TEXT,
    collapsed INTEGER NOT NULL DEFAULT 0,
    widget_only INTEGER NOT NULL DEFAULT 0,
    completed INTEGER NOT NULL DEFAULT 0,
    block_type TEXT NOT NULL DEFAULT 'text',
    created_at INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT 0,
    _change_origin TEXT,
    write_seq INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (parent_id) REFERENCES block_raw(id) DEFERRABLE INITIALLY DEFERRED
)";

const UNTYPED_ID: &str = "block:legacy-untyped";
const TYPED_ID: &str = "block:legacy-typed";
const RETYPED_ID: &str = "block:legacy-retyped";

fn service(env: &TestEnvironment) -> HolonService {
    HolonService::new_with_origin(
        env.engine().clone(),
        holon_api::OpOrigin::Agent {
            session_id: "mcp-session:legacy".to_string(),
            tool_call_id: "tool-call:legacy".to_string(),
        },
    )
}

fn create_params(id: &str) -> HashMap<Arc<str>, Value> {
    let mut params: HashMap<Arc<str>, Value> = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert(
        "parent_id".into(),
        Value::String("sentinel:no_parent".to_string()),
    );
    params.insert("content".into(), Value::String(id.to_string()));
    params
}

fn set_block_type(id: &str, value: Value) -> HashMap<Arc<str>, Value> {
    let mut params: HashMap<Arc<str>, Value> = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("field".into(), Value::String("block_type".to_string()));
    params.insert("value".into(), value);
    params
}

fn create_legacy_database(env: &TestEnvironment) {
    let db = holon_turso::turso::TursoBackend::open_database(env.temp_path().join("test.db"))
        .expect("opening the legacy database must succeed");
    db.connect()
        .expect("connecting to the legacy database must succeed")
        .execute(LEGACY_BLOCK_RAW)
        .expect("creating the legacy block_raw must succeed");
}

/// The stored `block_type` cell, and the typed slot the SQL row parses into.
async fn stored(env: &TestEnvironment, id: &str) -> (Option<Value>, Option<EntityName>) {
    let raw = env
        .query_sql(&format!(
            "SELECT block_type FROM block_raw WHERE id = '{id}'"
        ))
        .await
        .expect("reading block_raw must succeed");
    let raw = raw
        .first()
        .unwrap_or_else(|| panic!("no block_raw row for {id}"))
        .get("block_type")
        .cloned();
    let rows = env
        .engine()
        .execute_query(
            format!("SELECT * FROM block WHERE id = '{id}'"),
            HashMap::new(),
            None,
        )
        .await
        .expect("reading the block view must succeed");
    let row = rows
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no block view row for {id}"));
    let typed = Block::try_from(row)
        .unwrap_or_else(|e| panic!("{id}: the legacy row must parse: {e:#}"))
        .block_type;
    (raw, typed)
}

fn run_on_a_legacy_database(body: impl AsyncFnOnce(&TestEnvironment, &HolonService)) {
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    runtime.clone().block_on(async move {
        let env = TestEnvironment::new(runtime.clone()).unwrap();
        create_legacy_database(&env);
        env.start_app(true)
            .await
            .expect("start_app on a legacy block_raw");
        env.wait_for_loro_quiescence(Duration::from_secs(10)).await;
        let service = service(&env);
        body(&env, &service).await;
    });
}

/// Create `RETYPED_ID` untyped, then type it as `page` through `set_field`.
async fn create_then_type(env: &TestEnvironment, service: &HolonService) {
    let block = EntityName::new("block");
    service
        .execute_operation(&block, "create", create_params(RETYPED_ID))
        .await
        .unwrap_or_else(|e| panic!("the retyped block's create must land: {e:#}"));
    env.wait_for_loro_quiescence(Duration::from_secs(10)).await;
    service
        .execute_operation(
            &block,
            "set_field",
            set_block_type(RETYPED_ID, Value::String("page".to_string())),
        )
        .await
        .unwrap_or_else(|e| panic!("set_field(block_type) must land: {e:#}"));
    env.wait_for_loro_quiescence(Duration::from_secs(10)).await;
}

#[test]
fn block_type_reads_and_writes_on_a_legacy_block_raw() {
    run_on_a_legacy_database(async |env, service| {
        let block = EntityName::new("block");
        service
            .execute_operation(&block, "create", create_params(UNTYPED_ID))
            .await
            .unwrap_or_else(|e| panic!("an untyped create must land: {e:#}"));
        let mut typed = create_params(TYPED_ID);
        typed.insert("block_type".into(), Value::String("page".to_string()));
        service
            .execute_operation(&block, "create", typed)
            .await
            .unwrap_or_else(|e| panic!("a typed create must land: {e:#}"));
        create_then_type(env, service).await;

        let page = Some(EntityName::new("page"));
        let mut failures: Vec<String> = Vec::new();
        for (id, expected) in [
            (UNTYPED_ID, None),
            (TYPED_ID, page.clone()),
            (RETYPED_ID, page.clone()),
        ] {
            let (raw, typed) = stored(env, id).await;
            if typed != expected {
                failures.push(format!(
                    "{id}: block_type reads {typed:?} (stored {raw:?}), expected {expected:?}"
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    });
}

#[test]
#[ignore = "red: the projection's `UPDATE block_type = NULL` fails `NOT NULL constraint failed: \
            block_raw.block_type` on a legacy block_raw; lane stale-type rebuilds that table"]
fn clearing_block_type_on_a_legacy_block_raw() {
    run_on_a_legacy_database(async |env, service| {
        create_then_type(env, service).await;
        let clear = service
            .execute_operation(
                &EntityName::new("block"),
                "set_field",
                set_block_type(RETYPED_ID, Value::Null),
            )
            .await;

        let mut failures: Vec<String> = Vec::new();
        if let Err(e) = &clear {
            failures.push(format!("clearing block_type was refused: {e:#}"));
        }
        if let Err(why) = holon_loro_testing::quiescence::wait_for_loro_quiescence_on(
            env.loro_sync_handle().expect("Loro is enabled"),
            Duration::from_secs(10),
        )
        .await
        {
            failures.push(format!("the projection never applied the clear: {why}"));
        }
        let (raw, typed) = stored(env, RETYPED_ID).await;
        if typed.is_some() {
            failures.push(format!(
                "{RETYPED_ID}: a cleared block_type reads {typed:?} (stored {raw:?})"
            ));
        }
        let projection_errors = env.loro_sync_error_count();
        if projection_errors != 0 {
            failures.push(format!(
                "the Loro projection recorded {projection_errors} sink error(s)"
            ));
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    });
}
