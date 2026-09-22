//! An instance carries each template node's `block_type` and `completed` as
//! the write authority stores them, under both write authorities.
//!
//! Under Loro the SQL projection does not carry either column (bugfunnel
//! `2026-09-21-loro-path-drops-completed-and-block-type-before-sql`), so that
//! leg reads the Loro tree through its cells.
//!
//! @pbt kind harness
//! @pbt covers template-instantiate(stored-columns) — block_type/completed
//!   survive instantiation under Loro and SqlOnly

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityUri;
use holon_api::Value;
use holon_api::effect_id::deterministic_instance_id;
use holon_core::cell_registry::EntityCellRegistry;
use holon_core::cell_registry::EntityCellRegistryExt;
use holon_integration_tests::TestEnvironment;
use holon_integration_tests::TestEnvironmentBuilder;
use holon_loro::DocScope;
use holon_loro::block_cell_registry::BlockCellRegistry;

const TARGET: &str = "block:tsc-target";
const ROOT: &str = "block:tsc-root";
const CHILD: &str = "block:tsc-child";
const CONTEXT: &str = "tsc";

/// (template node, block_type, completed). Neither value is a create default.
const NODES: [(&str, &str, bool); 2] = [(ROOT, "note", true), (CHILD, "checklist", true)];

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
/// block_type, expected completed) per template node.
async fn instantiate(env: &TestEnvironment) -> Vec<(EntityUri, &'static str, bool)> {
    for (index, (id, block_type, completed)) in NODES.into_iter().enumerate() {
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
            ("completed", Value::Boolean(completed)),
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
        .map(|(id, block_type, completed)| {
            (
                deterministic_instance_id(ROOT, CONTEXT, id),
                block_type,
                completed,
            )
        })
        .collect()
}

fn assert_carried(observed: Vec<(EntityUri, String, bool)>, expected: &[(EntityUri, &str, bool)]) {
    let expected: Vec<(EntityUri, String, bool)> = expected
        .iter()
        .map(|(id, bt, done)| (id.clone(), bt.to_string(), *done))
        .collect();
    assert_eq!(
        observed, expected,
        "instance (id, block_type, completed) must equal the template node's"
    );
}

#[test]
fn loro_instance_carries_the_template_block_type_and_completed() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = boot(rt, true).await;
        let expected = instantiate(&env).await;
        let doc = env
            .loro_doc_store()
            .expect("Loro-enabled session")
            .read()
            .await
            .get_doc(DocScope::Global)
            .await
            .expect("global Loro doc");
        let registry: Box<dyn EntityCellRegistry> =
            Box::new(BlockCellRegistry::with_loro_doc(doc.doc()));
        let observed = expected
            .iter()
            .map(|(id, _, _)| {
                let block_type = registry
                    .as_ref()
                    .live_field::<String>(id, "block_type")
                    .unwrap_or_else(|e| panic!("block_type cell of {id}: {e:#}"))
                    .current();
                let completed = registry
                    .as_ref()
                    .live_field::<bool>(id, "completed")
                    .unwrap_or_else(|e| panic!("completed cell of {id}: {e:#}"))
                    .current();
                (id.clone(), block_type, completed)
            })
            .collect();
        assert_carried(observed, &expected);
    });
}

#[test]
fn sql_only_instance_carries_the_template_block_type_and_completed() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = boot(rt, false).await;
        let expected = instantiate(&env).await;
        let mut observed = Vec::new();
        for (id, _, _) in &expected {
            let rows = env
                .query_sql(&format!(
                    "SELECT block_type, completed FROM block_raw WHERE id = '{id}'"
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
            let completed = match row.get("completed") {
                Some(Value::Boolean(b)) => *b,
                Some(Value::Integer(0)) => false,
                Some(Value::Integer(1)) => true,
                other => panic!("{id}: completed is not a boolean: {other:?}"),
            };
            observed.push((id.clone(), block_type, completed));
        }
        assert_carried(observed, &expected);
    });
}
