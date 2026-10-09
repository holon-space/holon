//! Both write authorities read the same stored `block_type` into the same
//! `Block::block_type`.
//!
//! @pbt kind harness
//! @pbt covers template-instantiate(stored-columns) — one parse of the stored
//!   block_type shapes on the Loro and SqlOnly write authorities

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::Value;
use holon_core::WriteAuthorityReads;
use holon_integration_tests::TestEnvironment;
use holon_integration_tests::TestEnvironmentBuilder;

/// (shape name, stored `block_type` or none, expected slot).
fn shapes() -> Vec<(&'static str, Option<Value>, Option<EntityName>)> {
    vec![
        (
            "note",
            Some(Value::String("note".to_string())),
            Some(EntityName::new("note")),
        ),
        ("absent", None, None),
        ("legacy-text", Some(Value::String("text".to_string())), None),
    ]
}

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

async fn boot(rt: Arc<tokio::runtime::Runtime>, loro: bool) -> TestEnvironment {
    let builder = TestEnvironmentBuilder::new().with_org_file(
        "Shapes.org".to_string(),
        "#+TITLE: Shapes\n* Anchor\n:PROPERTIES:\n:ID: shapes-anchor\n:END:\n".to_string(),
    );
    let builder = if loro {
        builder
    } else {
        builder.without_loro()
    };
    let env = builder.build(rt).await.expect("boot the vault");
    if loro {
        env.wait_for_loro_quiescence(Duration::from_secs(60)).await;
    }
    env
}

async fn observe(env: &TestEnvironment, authority: &dyn WriteAuthorityReads) -> Vec<String> {
    let mut mismatches = Vec::new();
    for (name, block_type, expected) in shapes() {
        let id = format!("block:shape-{name}");
        let mut params: HashMap<String, Value> = [
            ("id", Value::String(id.clone())),
            (
                "parent_id",
                Value::String(EntityUri::no_parent().to_string()),
            ),
            ("content", Value::String(name.to_string())),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        if let Some(block_type) = block_type {
            params.insert("block_type".to_string(), block_type);
        }
        env.execute_operation("block", "create", params)
            .await
            .unwrap_or_else(|e| panic!("store shape {name}: {e:#}"));

        let got = authority
            .subtree(&EntityUri::parse(&id).expect("shape id"))
            .await
            .map(|nodes| nodes.expect("the shape block exists")[0].block_type.clone())
            .map_err(|e| e.to_string());
        if got.as_ref() != Ok(&expected) {
            mismatches.push(format!("{name}: expected {expected:?}, got {got:?}"));
        }
    }
    mismatches
}

#[test]
fn the_loro_authority_parses_each_stored_shape_like_the_shared_rule() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = boot(rt, true).await;
        let authority = env
            .injector()
            .expect("booted injector")
            .resolve::<dyn WriteAuthorityReads>();
        let mismatches = observe(&env, authority.as_ref()).await;
        assert!(
            mismatches.is_empty(),
            "Loro leg:\n{}",
            mismatches.join("\n")
        );
    });
}

#[test]
fn the_sql_authority_parses_each_stored_shape_like_the_shared_rule() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = boot(rt, false).await;
        let authority = holon::core::sql_write_authority::SqlWriteAuthority::new(
            env.engine().db_handle().clone(),
        );
        let mismatches = observe(&env, &authority).await;
        assert!(mismatches.is_empty(), "SQL leg:\n{}", mismatches.join("\n"));
    });
}
