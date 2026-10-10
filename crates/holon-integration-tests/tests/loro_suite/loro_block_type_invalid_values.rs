//! A typed block field (`block_type`, `collapsed`, `widget_only`) holding a
//! value its type cannot hold: an intent that would write one is refused, and
//! a store that already holds one still boots, reads the field as its default
//! and raises a `block_field_unreadable` condition until the value is fixed.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon::api::holon_service::HolonService;
use holon_api::Block;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::Value;
use holon_integration_tests::TestEnvironment;
use holon_loro::DocScope;
use holon_loro::LoroBackend;

const INVALID: &str = "not a name!";

fn service(env: &TestEnvironment) -> HolonService {
    HolonService::new_with_origin(
        env.engine().clone(),
        holon_api::OpOrigin::Agent {
            session_id: "mcp-session:bt-invalid".to_string(),
            tool_call_id: "tool-call:bt-invalid".to_string(),
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

fn set_field_params(id: &str, field: &str, value: Value) -> HashMap<Arc<str>, Value> {
    let mut params: HashMap<Arc<str>, Value> = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert("field".into(), Value::String(field.to_string()));
    params.insert("value".into(), value);
    params
}

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"))
}

async fn authority_block(env: &TestEnvironment, id: &str) -> Block {
    env.injector()
        .expect("booted injector")
        .resolve::<dyn holon_core::WriteAuthorityReads>()
        .block(&EntityUri::parse(id).expect("probe id"))
        .await
        .unwrap_or_else(|e| panic!("{id}: the authority read must succeed: {e:#}"))
        .unwrap_or_else(|| panic!("{id}: the authority holds no such block"))
}

async fn view_block(env: &TestEnvironment, id: &str) -> Block {
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
    Block::try_from(row).unwrap_or_else(|e| panic!("{id}: the block row must parse: {e:#}"))
}

/// The `block_field_unreadable` conditions currently raised, as
/// `(subject, Debug of the reason)`.
fn unreadable_conditions(env: &TestEnvironment) -> Vec<(String, String)> {
    env.injector()
        .expect("booted injector")
        .resolve::<Arc<holon_api::ConditionBus>>()
        .current()
        .into_iter()
        .filter(|c| c.condition_key().kind == "block-field-unreadable")
        .map(|c| (c.subject.clone(), format!("{:?}", c.reason)))
        .collect()
}

fn note_condition(
    env: &TestEnvironment,
    subject: &str,
    stored: &str,
    when: &str,
    failures: &mut Vec<String>,
) {
    let conditions = unreadable_conditions(env);
    if !conditions
        .iter()
        .any(|(s, reason)| s == subject && reason.contains(stored))
    {
        failures.push(format!(
            "{when}: no block_field_unreadable condition for {subject} naming {stored:?}; \
             raised: {conditions:?}"
        ));
    }
}

#[test]
fn intents_writing_an_invalid_block_type_are_refused() {
    refuses_invalid_intents(&[
        ("block_type", Value::String(INVALID.to_string())),
        ("block_type", Value::Integer(7)),
    ]);
}

#[test]
fn intents_writing_an_invalid_flag_are_refused() {
    refuses_invalid_intents(&[
        ("collapsed", Value::String("yes".to_string())),
        ("widget_only", Value::String("yes".to_string())),
        ("collapsed", Value::Integer(2)),
    ]);
}

fn refuses_invalid_intents(invalid_values: &[(&str, Value)]) {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = TestEnvironment::new(rt.clone()).unwrap();
        env.start_app(true).await.expect("start_app");
        env.wait_for_loro_quiescence(Duration::from_secs(10)).await;
        let service = service(&env);
        let block = EntityName::new("block");
        let target = "block:bt-invalid-target";
        service
            .execute_operation(&block, "create", create_params(target))
            .await
            .unwrap_or_else(|e| panic!("the target's create must land: {e:#}"));
        env.wait_for_loro_quiescence(Duration::from_secs(10)).await;

        let mut failures: Vec<String> = Vec::new();
        for (i, (field, value)) in invalid_values.iter().enumerate() {
            let shown = match value {
                Value::String(s) => s.clone(),
                other => format!("{other:?}"),
            };
            let set = service
                .execute_operation(
                    &block,
                    "set_field",
                    set_field_params(target, field, value.clone()),
                )
                .await;
            match set {
                Ok(_) => failures.push(format!("set_field({field}, {value:?}) was accepted")),
                Err(e) => {
                    let msg = format!("{e:#}");
                    if !msg.contains(field) || !msg.contains(&shown) {
                        failures.push(format!(
                            "set_field({field}, {value:?}): the refusal must name the field and \
                             the value: {msg}"
                        ));
                    }
                }
            }
            let created = format!("block:bt-invalid-create-{i}");
            let mut params = create_params(&created);
            params.insert((*field).into(), value.clone());
            if service
                .execute_operation(&block, "create", params)
                .await
                .is_ok()
            {
                failures.push(format!("create with {field} = {value:?} was accepted"));
            }
        }
        env.wait_for_loro_quiescence(Duration::from_secs(10)).await;

        let held = authority_block(&env, target).await;
        if held.block_type.is_some() || held.collapsed || held.widget_only {
            failures.push(format!(
                "the target changed: block_type {:?}, collapsed {}, widget_only {}",
                held.block_type, held.collapsed, held.widget_only
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

async fn loro_authority(env: &TestEnvironment) -> LoroBackend {
    let store = env
        .loro_doc_store()
        .expect("loro_doc_store present in Loro wiring")
        .clone();
    let store = store.read().await;
    let global = store.get_doc(DocScope::Global).await.expect("global doc");
    let layout = store.get_doc(DocScope::Layout).await.expect("layout doc");
    LoroBackend::from_document(global).with_layout_doc(layout)
}

/// Write `value` into `field` of block `id` straight into the Loro doc,
/// bypassing every intent boundary — the shape a peer or an older build can
/// leave behind.
async fn write_raw(env: &TestEnvironment, id: &str, field: &str, value: Value) {
    let mut props = HashMap::new();
    props.insert(field.to_string(), value);
    loro_authority(env)
        .await
        .update_block_properties(id, &props)
        .await
        .unwrap_or_else(|e| panic!("{id}: the raw Loro write must land: {e}"));
}

#[test]
fn a_loro_doc_holding_invalid_typed_fields_boots_reads_defaults_and_discloses() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt.clone()).unwrap();
        env.start_app(true).await.expect("start_app");
        env.wait_for_loro_quiescence(Duration::from_secs(10)).await;
        let service = service(&env);
        let block = EntityName::new("block");
        let probes = [
            ("block:bt-raw-type", "block_type", INVALID),
            ("block:bt-raw-collapsed", "collapsed", "yes"),
            ("block:bt-raw-widget", "widget_only", "yes"),
        ];
        for (id, _, _) in probes {
            service
                .execute_operation(&block, "create", create_params(id))
                .await
                .unwrap_or_else(|e| panic!("{id}: create must land: {e:#}"));
        }
        env.wait_for_loro_quiescence(Duration::from_secs(10)).await;
        for (id, field, stored) in probes {
            write_raw(&env, id, field, Value::String(stored.to_string())).await;
        }
        env.wait_for_loro_quiescence(Duration::from_secs(10)).await;

        let mut failures: Vec<String> = Vec::new();
        for (id, field, stored) in probes {
            note_condition(
                &env,
                &format!("{id}/{field}"),
                stored,
                "live",
                &mut failures,
            );
        }

        env.stop_app().await.expect("stop_app");
        env.start_app(true)
            .await
            .expect("restart on the invalid doc");
        env.wait_for_loro_quiescence(Duration::from_secs(10)).await;
        for (id, field, stored) in probes {
            note_condition(
                &env,
                &format!("{id}/{field}"),
                stored,
                "after restart",
                &mut failures,
            );
            let held = authority_block(&env, id).await;
            let projected = view_block(&env, id).await;
            for (leg, b) in [("authority", &held), ("SQL view", &projected)] {
                if b.block_type.is_some() || b.collapsed || b.widget_only {
                    failures.push(format!(
                        "{id} via {leg}: expected defaults, got block_type {:?}, collapsed {}, \
                         widget_only {}",
                        b.block_type, b.collapsed, b.widget_only
                    ));
                }
            }
        }
        let projection_errors = env.loro_sync_error_count();
        if projection_errors != 0 {
            failures.push(format!(
                "the Loro projection recorded {projection_errors} sink error(s)"
            ));
        }

        let service = self::service(&env);
        let fixes = [
            ("block:bt-raw-type", "block_type", Value::Null),
            ("block:bt-raw-collapsed", "collapsed", Value::Boolean(true)),
            ("block:bt-raw-widget", "widget_only", Value::Boolean(false)),
        ];
        for (id, field, value) in fixes {
            service
                .execute_operation(&block, "set_field", set_field_params(id, field, value))
                .await
                .unwrap_or_else(|e| panic!("{id}: fixing {field} must land: {e:#}"));
        }
        env.wait_for_loro_quiescence(Duration::from_secs(10)).await;
        let left = unreadable_conditions(&env);
        if !left.is_empty() {
            failures.push(format!("conditions left after the fix: {left:?}"));
        }
        if !authority_block(&env, "block:bt-raw-collapsed")
            .await
            .collapsed
        {
            failures.push("the collapsed fix did not land".to_string());
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    });
}

#[test]
fn an_org_drawer_naming_block_type_boots_and_reads_untyped() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = TestEnvironment::new(rt.clone()).unwrap();
        env.write_org_file(
            "drawer.org",
            "* drawer probe\n:PROPERTIES:\n:ID: bt-drawer-probe\n:block_type: not a name!\n:END:\n",
        )
        .await
        .expect("write drawer.org");
        env.start_app(true)
            .await
            .expect("start_app with the drawer");
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while env
            .query_sql("SELECT id FROM block_raw WHERE id = 'block:bt-drawer-probe'")
            .await
            .expect("query block_raw")
            .is_empty()
        {
            assert!(
                std::time::Instant::now() < deadline,
                "the drawer probe never reached SQL"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        env.wait_for_loro_quiescence(Duration::from_secs(10)).await;
        let projected = view_block(&env, "block:bt-drawer-probe").await;
        assert_eq!(
            projected.block_type, None,
            "the drawer key must not type it"
        );
        let held = authority_block(&env, "block:bt-drawer-probe").await;
        assert_eq!(held.block_type, None, "the drawer key must not type it");
        let mut failures: Vec<String> = Vec::new();
        note_condition(
            &env,
            "block:bt-drawer-probe/block_type",
            INVALID,
            "boot",
            &mut failures,
        );
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    });
}

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

#[test]
fn a_sql_row_holding_an_invalid_block_type_boots_reads_untyped_and_discloses() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = TestEnvironment::new(rt.clone()).unwrap();
        env.set_enable_loro(false);
        let id = "block:bt-sql-invalid";
        let db = holon_turso::turso::TursoBackend::open_database(env.temp_path().join("test.db"))
            .expect("opening the database must succeed");
        let conn = db.connect().expect("connecting must succeed");
        conn.execute(LEGACY_BLOCK_RAW)
            .expect("creating the legacy block_raw must succeed");
        conn.execute(&format!(
            "INSERT INTO block_raw (id, parent_id, content, block_type) VALUES ('{id}', NULL, \
             'sql invalid', '{INVALID}')"
        ))
        .expect("inserting the invalid row must succeed");
        drop(conn);
        drop(db);

        env.start_app(true)
            .await
            .expect("start_app on a row with an invalid block_type");
        let mut failures: Vec<String> = Vec::new();
        note_condition(
            &env,
            &format!("{id}/block_type"),
            INVALID,
            "boot",
            &mut failures,
        );
        let held = holon_core::WriteAuthorityReads::block(
            &holon::core::sql_write_authority::SqlWriteAuthority::new(
                env.engine().db_handle().clone(),
            ),
            &EntityUri::parse(id).expect("probe id"),
        )
        .await
        .expect("the SQL authority read must succeed")
        .expect("the SQL authority holds the row");
        if held.block_type.is_some() {
            failures.push(format!("authority reads block_type {:?}", held.block_type));
        }
        let projected = view_block(&env, id).await;
        if projected.block_type.is_some() {
            failures.push(format!(
                "SQL view reads block_type {:?}",
                projected.block_type
            ));
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    });
}
