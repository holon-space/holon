//! F-C probe: do `completed` and `block_type` — two first-class `block_raw`
//! COLUMNS — survive the CRDT path?
//!
//! `BlockCellRegistry::write_field` routes
//! `set_field("completed"|"block_type")` into the Loro meta property map
//! (`crates/holon-loro/src/block_cell_registry.rs:1045-1057`, the `_` scalar
//! arm), but `read_properties_from_meta` strips both keys out again via
//! `RESERVED_PROPERTY_KEYS` (`crates/holon-loro/src/loro_backend.rs:458`,
//! `:486-487`), `Block` has no typed slot for either, and `block_to_params`
//! (`crates/holon-loro/src/loro_sync_controller.rs:2163-2273`) never emits
//! them. If that reading is right, the authority holds the value and the
//! projection drops it before SQL — for every write on the Loro path.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityName;
use holon_api::Value;
use holon_integration_tests::test_environment::TestEnvironment;

const CREATE_ID: &str = "block:fc-create-probe";
const SET_FIELD_ID: &str = "block:fc-set-field-probe";

fn service(env: &TestEnvironment) -> holon::api::holon_service::HolonService {
    holon::api::holon_service::HolonService::new_with_origin(
        env.engine().clone(),
        holon_api::OpOrigin::Agent {
            session_id: "mcp-session:fc".to_string(),
            tool_call_id: "tool-call:fc".to_string(),
        },
    )
}

/// Both columns must carry the authored value into `block_raw` — once at
/// create, and again after a `set_field` on a block that already exists.
///
/// KNOWN RED, `#[ignore]`d so it does not fail a land gate (the D66.a
/// pattern); run it with `--run-ignored all`. It reports 4 of 4 authored
/// writes dropped. The fix is a ruling, not a patch — `block_type` needs a
/// typed `Block` slot and an explicit `block_to_params` emission (the
/// `collapsed`/`widget_only` precedent), and `completed` needs a decision on
/// whether the column is alive at all. Tracked as
/// `docs/Testing/bugfunnel/entries/
/// 2026-09-21-loro-path-drops-completed-and-block-type-before-sql.md`.
#[test]
#[ignore = "known red: the Loro path drops 4 of 4 authored completed/block_type column writes before SQL — see the bugfunnel entry"]
fn completed_and_block_type_survive_the_loro_path() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    runtime.clone().block_on(async move {
        let env = TestEnvironment::new(runtime.clone()).unwrap();
        env.start_app(true).await.expect("start_app");
        env.wait_for_loro_quiescence(Duration::from_secs(10)).await;
        let service = service(&env);

        // Leg 1 — authored at CREATE.
        let mut params: HashMap<Arc<str>, Value> = HashMap::new();
        params.insert("id".into(), Value::String(CREATE_ID.to_string()));
        params.insert(
            "parent_id".into(),
            Value::String("sentinel:no_parent".to_string()),
        );
        params.insert("content".into(), Value::String("fc create".to_string()));
        params.insert("block_type".into(), Value::String("page".to_string()));
        params.insert("completed".into(), Value::Boolean(true));
        service
            .execute_operation(&EntityName::new("block"), "create", params)
            .await
            .unwrap_or_else(|e| panic!("the F-C create probe must land: {e:#}"));

        // Leg 2 — written afterwards through `set_field`, the widget path.
        let mut params: HashMap<Arc<str>, Value> = HashMap::new();
        params.insert("id".into(), Value::String(SET_FIELD_ID.to_string()));
        params.insert(
            "parent_id".into(),
            Value::String("sentinel:no_parent".to_string()),
        );
        params.insert("content".into(), Value::String("fc set_field".to_string()));
        service
            .execute_operation(&EntityName::new("block"), "create", params)
            .await
            .unwrap_or_else(|e| panic!("the F-C set_field probe's create must land: {e:#}"));
        env.wait_for_loro_quiescence(Duration::from_secs(10)).await;

        for (field, value) in [
            ("completed", Value::Boolean(true)),
            ("block_type", Value::String("page".to_string())),
        ] {
            let mut params: HashMap<Arc<str>, Value> = HashMap::new();
            params.insert("id".into(), Value::String(SET_FIELD_ID.to_string()));
            params.insert("field".into(), Value::String(field.to_string()));
            params.insert("value".into(), value);
            service
                .execute_operation(&EntityName::new("block"), "set_field", params)
                .await
                .unwrap_or_else(|e| panic!("set_field({field}) must land: {e:#}"));
        }
        env.wait_for_loro_quiescence(Duration::from_secs(10)).await;

        // Every observation is collected before anything is asserted, so one
        // run reports all four cells rather than stopping at the first.
        let mut observed: Vec<String> = Vec::new();
        let mut lost: Vec<String> = Vec::new();
        for (id, leg, content) in [
            (CREATE_ID, "create", "fc create"),
            (SET_FIELD_ID, "set_field", "fc set_field"),
        ] {
            let rows = env
                .query_sql(&format!(
                    "SELECT content, completed, block_type FROM block_raw WHERE id = '{id}'"
                ))
                .await
                .expect("reading the projected row must succeed");
            let row = rows
                .first()
                .unwrap_or_else(|| panic!("no block_raw row for {id} ({leg} leg)"));

            // Anti-vacuity: a column the projection DOES carry must have
            // arrived, so a failure below means "this column was dropped",
            // never "the write never happened".
            assert_eq!(
                row.get("content"),
                Some(&Value::String(content.to_string())),
                "{leg} leg: the control column `content` did not arrive, so this test proves \
                 nothing about the two columns under study"
            );

            let bt = row.get("block_type");
            let done = row.get("completed");
            observed.push(format!("{leg}: block_type={bt:?} completed={done:?}"));
            if bt != Some(&Value::String("page".to_string())) {
                lost.push(format!("{leg}.block_type"));
            }
            if !matches!(done, Some(Value::Boolean(true)) | Some(Value::Integer(1))) {
                lost.push(format!("{leg}.completed"));
            }
        }
        assert!(
            lost.is_empty(),
            "the Loro path dropped {} of 4 authored column writes before SQL: {}\nobserved: {}",
            lost.len(),
            lost.join(", "),
            observed.join(" · ")
        );
    });
}
