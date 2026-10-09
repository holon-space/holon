//! An authored `block_type` reaches `block_raw` on the Loro path — at create
//! and through `set_field` — and `completed` is no block field at all.

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

fn create_params(id: &str, content: &str) -> HashMap<Arc<str>, Value> {
    let mut params: HashMap<Arc<str>, Value> = HashMap::new();
    params.insert("id".into(), Value::String(id.to_string()));
    params.insert(
        "parent_id".into(),
        Value::String("sentinel:no_parent".to_string()),
    );
    params.insert("content".into(), Value::String(content.to_string()));
    params
}

fn set_field_params(field: &str, value: Value) -> HashMap<Arc<str>, Value> {
    let mut params: HashMap<Arc<str>, Value> = HashMap::new();
    params.insert("id".into(), Value::String(SET_FIELD_ID.to_string()));
    params.insert("field".into(), Value::String(field.to_string()));
    params.insert("value".into(), value);
    params
}

#[test]
fn block_type_survives_the_loro_path_and_completed_is_gone() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    runtime.clone().block_on(async move {
        let env = TestEnvironment::new(runtime.clone()).unwrap();
        env.start_app(true).await.expect("start_app");
        env.wait_for_loro_quiescence(Duration::from_secs(10)).await;
        let service = service(&env);

        let mut params = create_params(CREATE_ID, "fc create");
        params.insert("block_type".into(), Value::String("page".to_string()));
        service
            .execute_operation(&EntityName::new("block"), "create", params)
            .await
            .unwrap_or_else(|e| panic!("the create probe must land: {e:#}"));

        service
            .execute_operation(
                &EntityName::new("block"),
                "create",
                create_params(SET_FIELD_ID, "fc set_field"),
            )
            .await
            .unwrap_or_else(|e| panic!("the set_field probe's create must land: {e:#}"));
        env.wait_for_loro_quiescence(Duration::from_secs(10)).await;
        service
            .execute_operation(
                &EntityName::new("block"),
                "set_field",
                set_field_params("block_type", Value::String("page".to_string())),
            )
            .await
            .unwrap_or_else(|e| panic!("set_field(block_type) must land: {e:#}"));
        let completed_write = service
            .execute_operation(
                &EntityName::new("block"),
                "set_field",
                set_field_params("completed", Value::Boolean(true)),
            )
            .await;
        env.wait_for_loro_quiescence(Duration::from_secs(10)).await;

        // Every cell is observed before anything is asserted, so one run
        // reports them all.
        let mut failures: Vec<String> = Vec::new();
        for (id, leg, content) in [
            (CREATE_ID, "create", "fc create"),
            (SET_FIELD_ID, "set_field", "fc set_field"),
        ] {
            let rows = env
                .query_sql(&format!(
                    "SELECT content, block_type FROM block_raw WHERE id = '{id}'"
                ))
                .await
                .expect("reading the projected row must succeed");
            let row = rows
                .first()
                .unwrap_or_else(|| panic!("no block_raw row for {id} ({leg} leg)"));
            // `content` is a column the projection carries, so its arrival
            // proves the write happened.
            assert_eq!(
                row.get("content"),
                Some(&Value::String(content.to_string())),
                "{leg} leg: the control column `content` did not arrive"
            );
            let bt = row.get("block_type");
            if bt != Some(&Value::String("page".to_string())) {
                failures.push(format!("{leg}: block_type reached SQL as {bt:?}"));
            }
        }

        let authority = env
            .injector()
            .expect("booted injector")
            .resolve::<dyn holon_core::WriteAuthorityReads>();
        let held = authority
            .block(&holon_api::EntityUri::parse(SET_FIELD_ID).expect("probe id"))
            .await
            .expect("the authority read must succeed")
            .expect("the authority holds the probe block");
        if held.block_type != Some(EntityName::new("page")) {
            failures.push(format!(
                "set_field: the authority holds block_type {:?}",
                held.block_type
            ));
        }

        let columns = env
            .query_sql("SELECT name FROM pragma_table_info('block_raw')")
            .await
            .expect("reading block_raw's columns must succeed");
        assert!(
            columns
                .iter()
                .any(|c| c.get("name") == Some(&Value::String("block_type".to_string()))),
            "pragma_table_info must list block_raw's columns: {columns:?}"
        );
        if columns
            .iter()
            .any(|c| c.get("name") == Some(&Value::String("completed".to_string())))
        {
            failures.push("block_raw still has a `completed` column".to_string());
        }
        if completed_write.is_ok() {
            failures.push("set_field(completed) was accepted".to_string());
        }

        assert!(failures.is_empty(), "{}", failures.join("\n"));
    });
}
