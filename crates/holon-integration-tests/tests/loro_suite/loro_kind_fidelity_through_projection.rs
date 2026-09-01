//! S2 end-to-end lock: the kind a property is WRITTEN with must survive the
//! whole production path — prod session → Loro CRUD authority → Loro→SQL
//! projection → the SQL read boundary.
//!
//! The unit-level pins prove each half separately: `holon-loro`'s
//! `date_time_and_json_keep_their_kind_across_a_restart` proves the Loro
//! on-disk form, and `holon`'s `the_loro_leg_keeps_every_declared_kind` proves
//! the leg against the profile. Neither proves the JOIN — that a `DateTime`
//! written through the real session reaches `block_raw` with
//! `property_kinds` recording it, so a later read gives the kind back.
//!
//! That join was argued from code (`loro_sync_controller.rs` passes typed
//! `Value`s through and `SqlOperationProvider` re-derives the kinds) rather
//! than measured, and an argued through-line is exactly the kind that rots
//! silently. This measures it.
//!
//! `Value` is `#[serde(untagged)]`, so before S2 the Loro leg answered
//! `String` for both kinds and the projection faithfully carried the `String`
//! onward. Disabling the envelope branch in
//! `holon-loro/src/loro_backend.rs::encode_property_value` turns this red.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityName;
use holon_api::PropertyKinds;
use holon_api::Value;
use holon_integration_tests::test_environment::TestEnvironment;

const BLOCK_ID: &str = "block:s2-kind-through-projection";
const WHEN: &str = "2026-08-22T10:00:00Z";
const DOC: &str = r#"{"a":1}"#;

/// A `DateTime` and a `Json` written through the live session keep their kind
/// all the way into `block_raw`, and a look-alike `String` does not acquire
/// one.
#[test]
fn ambiguous_kinds_survive_the_loro_to_sql_projection() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    runtime.clone().block_on(async move {
        let env = TestEnvironment::new(runtime.clone()).unwrap();
        env.start_app(true).await.expect("start_app");
        env.wait_for_loro_quiescence(Duration::from_secs(10)).await;

        // The same facade the MCP server builds — the live authoring path, so
        // the create routes to the Loro CRUD authority and reaches SQL only by
        // projection.
        let service = holon::api::holon_service::HolonService::new_with_origin(
            env.engine().clone(),
            holon_api::OpOrigin::Agent {
                session_id: "mcp-session:s2".to_string(),
                tool_call_id: "tool-call:s2".to_string(),
            },
        );

        let mut params: HashMap<Arc<str>, Value> = HashMap::new();
        params.insert("id".into(), Value::String(BLOCK_ID.to_string()));
        params.insert(
            "parent_id".into(),
            Value::String("sentinel:no_parent".to_string()),
        );
        params.insert("content".into(), Value::String("s2 probe".to_string()));
        params.insert("when".into(), Value::DateTime(WHEN.to_string()));
        params.insert("doc".into(), Value::Json(DOC.to_string()));
        // CONTROL: byte-identical to `when`, but authored as a plain string.
        // The kind must record what the AUTHOR meant, so this one must not be
        // swept into `date_time` by a reader that sniffs the text.
        params.insert("plain".into(), Value::String(WHEN.to_string()));

        service
            .execute_operation(&EntityName::new("block"), "create", params)
            .await
            .unwrap_or_else(|e| panic!("the S2 probe's create must land: {e:#}"));

        env.wait_for_loro_quiescence(Duration::from_secs(10)).await;

        let sql =
            format!("SELECT properties, property_kinds FROM block_raw WHERE id = '{BLOCK_ID}'");
        let rows = env
            .query_sql(&sql)
            .await
            .expect("reading the projected row must succeed");
        let row = rows.first().unwrap_or_else(|| {
            panic!("the projection never produced a block_raw row for {BLOCK_ID}")
        });

        // 1. The kinds column records BOTH ambiguous keys and ONLY those.
        // Compared as a parsed `PropertyKinds` rather than as text, so the
        // assertion cannot pass on a coincidentally-similar spelling.
        let stored = PropertyKinds::parse_column(row.get("property_kinds"))
            .expect("property_kinds must parse");
        let expected = PropertyKinds::of([
            ("when", &Value::DateTime(WHEN.to_string())),
            ("doc", &Value::Json(DOC.to_string())),
            ("plain", &Value::String(WHEN.to_string())),
        ]);
        assert_eq!(
            stored, expected,
            "the projection must carry the authored kinds into block_raw.property_kinds; \
             got {stored:?}"
        );

        // 2. …and the read boundary gives them back TYPED. This is the half a
        // kinds column alone would not prove: a recorded kind nothing restores
        // is a kind the reader never sees.
        let bag = match row.get("properties") {
            Some(Value::Object(map)) => map.clone(),
            other => panic!("the properties column came back as {other:?}"),
        };
        assert_eq!(
            bag.get("when"),
            Some(&Value::DateTime(WHEN.to_string())),
            "`when` must read back a DateTime after the full projection"
        );
        assert_eq!(
            bag.get("doc"),
            Some(&Value::Json(DOC.to_string())),
            "`doc` must read back a Json document after the full projection"
        );
        assert_eq!(
            bag.get("plain"),
            Some(&Value::String(WHEN.to_string())),
            "an authored plain string must NOT be re-typed into a DateTime"
        );
    });
}
