//! An instance carries each template node's `block_type` into `block_raw`,
//! under both write authorities.
//!
//! @pbt kind harness
//! @pbt covers template-instantiate(stored-columns) — block_type survives
//!   instantiation under Loro and SqlOnly

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityUri;
use holon_api::Value;
use holon_api::effect_id::deterministic_instance_id;
use holon_integration_tests::TestEnvironment;
use holon_integration_tests::TestEnvironmentBuilder;

const TARGET: &str = "block:tsc-target";
const ROOT: &str = "block:tsc-root";
const CHILD: &str = "block:tsc-child";
const CONTEXT: &str = "tsc";

/// (template node, block_type).
const NODES: [(&str, &str); 2] = [(ROOT, "note"), (CHILD, "checklist")];

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

fn params(pairs: Vec<(&str, Value)>) -> HashMap<String, Value> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

async fn settle(env: &TestEnvironment) {
    if env.loro_enabled() {
        env.wait_for_loro_quiescence(Duration::from_secs(60)).await;
    }
    env.wait_for_cdc_quiescent(Duration::from_millis(300), Duration::from_secs(60))
        .await;
}

async fn boot(rt: Arc<tokio::runtime::Runtime>, loro: bool) -> TestEnvironment {
    let builder = TestEnvironmentBuilder::new().with_org_file(
        "Tsc.org".to_string(),
        "#+TITLE: Tsc\n* Target\n:PROPERTIES:\n:ID: tsc-target\n:END:\n".to_string(),
    );
    let builder = if loro {
        builder
    } else {
        builder.without_loro()
    };
    let env = builder.build(rt).await.expect("boot the vault");
    settle(&env).await;
    env
}

/// Writes the template and instantiates it; returns (instance id, expected
/// block_type) per template node.
async fn instantiate(env: &TestEnvironment) -> Vec<(EntityUri, String)> {
    for (index, (id, block_type)) in NODES.into_iter().enumerate() {
        let parent = if index == 0 {
            EntityUri::no_parent().to_string()
        } else {
            ROOT.to_string()
        };
        let mut fields = vec![
            ("id", Value::String(id.to_string())),
            ("parent_id", Value::String(parent)),
            ("content", Value::String(format!("{id} content"))),
            ("block_type", Value::String(block_type.to_string())),
        ];
        if index == 0 {
            fields.push(("template", Value::String("t".to_string())));
            fields.push(("template_vars", Value::String(String::new())));
        }
        env.execute_operation("block", "create", params(fields))
            .await
            .unwrap_or_else(|e| panic!("create template node {id}: {e:#}"));
    }
    env.execute_operation(
        "block",
        "instantiate_template",
        params(vec![
            ("template_id", Value::String(ROOT.to_string())),
            ("target_parent", Value::String(TARGET.to_string())),
            ("context_key", Value::String(CONTEXT.to_string())),
        ]),
    )
    .await
    .expect("instantiate the template");
    settle(env).await;
    NODES
        .into_iter()
        .map(|(id, block_type)| {
            (
                deterministic_instance_id(ROOT, CONTEXT, id),
                block_type.to_string(),
            )
        })
        .collect()
}

async fn stored_block_types(
    env: &TestEnvironment,
    expected: &[(EntityUri, String)],
) -> Vec<(EntityUri, String)> {
    let mut observed = Vec::new();
    for (id, _) in expected {
        let rows = env
            .query_sql(&format!(
                "SELECT block_type FROM block_raw WHERE id = '{id}'"
            ))
            .await
            .expect("query the instance row");
        let row = rows
            .first()
            .unwrap_or_else(|| panic!("no block_raw row for instance {id}"));
        let block_type = row
            .get("block_type")
            .and_then(|v| v.as_string())
            .unwrap_or_else(|| panic!("{id}: block_type is not a string: {row:?}"))
            .to_string();
        observed.push((id.clone(), block_type));
    }
    observed
}

#[test]
fn loro_instance_carries_the_template_block_type() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = boot(rt, true).await;
        let expected = instantiate(&env).await;
        assert_eq!(stored_block_types(&env, &expected).await, expected);
    });
}

#[test]
fn sql_only_instance_carries_the_template_block_type() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = boot(rt, false).await;
        let expected = instantiate(&env).await;
        assert_eq!(stored_block_types(&env, &expected).await, expected);
    });
}
