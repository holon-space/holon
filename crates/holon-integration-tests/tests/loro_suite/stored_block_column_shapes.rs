//! Both write authorities turn the same stored `completed` / `block_type` into
//! the same `StoredBlock`, or refuse it alike.
//!
//! Only shapes each store keeps as written are fed in: SQL's column affinity
//! rewrites a non-text `block_type` as text, so a mistyped `block_type` cannot
//! reach the SQL leg at all.
//!
//! @pbt kind harness
//! @pbt covers template-instantiate(stored-columns) — one parse of the stored
//!   block_type/completed shapes on the Loro and SqlOnly write authorities

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityUri;
use holon_api::Value;
use holon_core::WriteAuthorityReads;
use holon_integration_tests::TestEnvironment;
use holon_integration_tests::TestEnvironmentBuilder;

/// Expected parse of one stored shape: `Ok((block_type, completed))` or an
/// error whose message names the column.
type Outcome = Result<(Option<String>, Option<bool>), &'static str>;

fn shapes() -> Vec<(&'static str, Value, Value, Outcome)> {
    let note = || Value::String("note".to_string());
    let ok = |done: bool| Ok((Some("note".to_string()), Some(done)));
    vec![
        ("bool-true", note(), Value::Boolean(true), ok(true)),
        ("bool-false", note(), Value::Boolean(false), ok(false)),
        ("int-one", note(), Value::Integer(1), ok(true)),
        ("int-zero", note(), Value::Integer(0), ok(false)),
        ("int-two", note(), Value::Integer(2), Err("completed")),
        (
            "text-yes",
            note(),
            Value::String("yes".to_string()),
            Err("completed"),
        ),
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
    for (name, block_type, completed, expected) in shapes() {
        let id = format!("block:shape-{name}");
        let params: HashMap<String, Value> = [
            ("id", Value::String(id.clone())),
            (
                "parent_id",
                Value::String(EntityUri::no_parent().to_string()),
            ),
            ("content", Value::String(name.to_string())),
            ("block_type", block_type),
            ("completed", completed),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        env.execute_operation("block", "create", params)
            .await
            .unwrap_or_else(|e| panic!("store shape {name}: {e:#}"));

        let got = authority
            .subtree(&EntityUri::parse(&id).expect("shape id"))
            .await
            .map(|nodes| {
                let node = &nodes.expect("the shape block exists")[0];
                (node.block_type.clone(), node.completed)
            })
            .map_err(|e| e.to_string());
        let agrees = match (&got, &expected) {
            (Ok(g), Ok(e)) => g == e,
            (Err(msg), Err(column)) => msg.contains(column),
            _ => false,
        };
        if !agrees {
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
