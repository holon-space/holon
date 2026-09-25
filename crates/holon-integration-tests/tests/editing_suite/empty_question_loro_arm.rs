//! The engine's `?`-question write rules on the Loro arm, booted from org
//! files: a `?` over empty content is refused, the cycle skips `?` on a block
//! with no text, and emptying a question's text drops the `?`.
//!
//! @pbt kind harness
//! @pbt covers empty-question-write-boundary — the three engine write rules
//!   hold when Loro holds block CRUD

use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_integration_tests::TestEnvironment;

const DECISIONS_ORG: &str = "\
#+TITLE: Decisions
#+ID: page-decisions
#+TODO: TODO ? | DONE

* placeholder
:PROPERTIES:
:ID: q-empty
:END:
* pick a storage engine
:PROPERTIES:
:ID: q-asks
:END:
";

const EMPTY_ID: &str = "block:q-empty";
const ASKS_ID: &str = "block:q-asks";

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

async fn booted(rt: Arc<tokio::runtime::Runtime>) -> TestEnvironment {
    let env = TestEnvironment::new(rt).expect("TestEnvironment::new");
    env.write_org_file("decisions.org", DECISIONS_ORG)
        .await
        .expect("write decisions.org");
    env.start_app(true).await.expect("start_app");
    assert!(env.loro_enabled(), "this suite pins the Loro arm");
    let deadline = std::time::Instant::now() + Duration::from_secs(25);
    for id in [EMPTY_ID, ASKS_ID] {
        while env
            .query_sql(&format!("SELECT id FROM block_raw WHERE id = '{id}'"))
            .await
            .expect("query block_raw")
            .is_empty()
        {
            assert!(
                std::time::Instant::now() < deadline,
                "org scan never populated {id}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    run(
        &env,
        "set_field",
        &[("id", EMPTY_ID), ("field", "content"), ("value", "")],
    )
    .await
    .expect("empty the placeholder");
    settle(&env).await;
    env
}

async fn settle(env: &TestEnvironment) {
    env.wait_for_loro_quiescence(Duration::from_secs(15)).await;
    env.wait_for_cdc_quiescent(Duration::from_millis(250), Duration::from_secs(15))
        .await;
}

async fn run(env: &TestEnvironment, op: &str, params: &[(&str, &str)]) -> anyhow::Result<()> {
    let mut p = holon_api::StorageEntity::new();
    for (k, v) in params {
        p.insert((*k).into(), Value::String((*v).to_string()));
    }
    env.engine()
        .execute_operation(&EntityName::new("block"), op, p, OpOrigin::User)
        .await
        .map(|_| ())
}

async fn task_state(env: &TestEnvironment, id: &str) -> String {
    settle(env).await;
    let rows = env
        .query_sql(&format!(
            "SELECT json_extract(properties, '$.task_state') AS ts FROM block_raw WHERE id = '{id}'"
        ))
        .await
        .expect("query block_raw");
    rows.first()
        .unwrap_or_else(|| panic!("{id} missing from block_raw"))
        .get("ts")
        .and_then(|v| v.as_string())
        .unwrap_or_default()
        .to_string()
}

#[test]
fn setting_the_question_keyword_on_an_empty_block_is_refused() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = booted(rt.clone()).await;
        let err = run(
            &env,
            "set_field",
            &[("id", EMPTY_ID), ("field", "task_state"), ("value", "?")],
        )
        .await
        .expect_err("an empty question must be refused");
        assert!(
            format!("{err:#}").contains(EMPTY_ID),
            "the refusal must name the block: {err:#}"
        );
        assert_eq!(task_state(&env, EMPTY_ID).await, "");
    });
}

#[test]
fn the_cycle_skips_the_question_on_an_empty_block() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = booted(rt.clone()).await;
        let mut seen = Vec::new();
        for _ in 0..3 {
            run(&env, "cycle_task_state", &[("id", EMPTY_ID)])
                .await
                .expect("cycle_task_state");
            seen.push(task_state(&env, EMPTY_ID).await);
        }
        assert_eq!(seen, vec!["TODO", "DONE", ""]);
    });
}

#[test]
fn clearing_a_questions_text_drops_the_question() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = booted(rt.clone()).await;
        run(
            &env,
            "set_field",
            &[("id", ASKS_ID), ("field", "task_state"), ("value", "?")],
        )
        .await
        .expect("a question with text is legal");
        assert_eq!(task_state(&env, ASKS_ID).await, "?");
        run(
            &env,
            "set_field",
            &[("id", ASKS_ID), ("field", "content"), ("value", "")],
        )
        .await
        .expect("clearing the text is a legal edit");
        assert_eq!(task_state(&env, ASKS_ID).await, "");
    });
}
