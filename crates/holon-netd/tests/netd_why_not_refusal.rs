//! `why_not` over the MCP endpoint: a transition the net does not hold is
//! refused by name and leaves netd answering; a transition it holds answers
//! for any subject.

mod common;

use std::fs;
use std::path::Path;
use std::path::PathBuf;

use common::Cli;
use common::Process;
use proptest::prelude::*;
use proptest::test_runner::Config;
use proptest::test_runner::TestCaseError;
use proptest::test_runner::TestRunner;
use serde_json::json;

const HELD: [&str; 2] = ["Pay", "Ship"];

/// `Pay` is pure, `Ship` runs `true`, so the executor also keeps its own
/// places for `Ship`.
fn net(dir: &Path) -> PathBuf {
    let dir = dir.display();
    let text = format!(
        r#"name: shop
places:
  Order:
    capacity: single
  Paid:
    capacity: single
  Shipped:
    capacity: single
transitions:
  Pay:
    trigger: manual
    priority: 1
    consumes:
    - Order
    produces:
    - Paid
  Ship:
    trigger: manual
    priority: 1
    consumes:
    - Paid
    produces:
    - Shipped
    effect: ship
    receiver: none
effects:
  ship:
    kind: shell
    command:
      argv:
      - /usr/bin/true
      working_dir: {dir}
      env_allow: []
      timeout: 10s
    log_path: logs/{{transition}}-{{subject}}-{{attempt}}.log
    exit_codes:
      done:
      - 0
      failed: []
      other: outcome_unknown
"#
    );
    let path = PathBuf::from(format!("{dir}/shop.yaml"));
    fs::write(&path, text).expect("the net file is writable");
    path
}

/// Any string, and the names a caller confuses with a held transition: a
/// changed case or blank, a lowered form, an executor-owned place.
fn unheld() -> impl Strategy<Value = String> {
    prop_oneof![
        any::<String>(),
        Just(String::new()),
        prop::sample::select(vec![
            "pay",
            "PAY",
            "Pay ",
            " Ship",
            "Answer.Pay",
            "Ship.Running",
            "Ship.RetryPending",
            "Ship.OutcomeUnknown",
            "OutcomeUnknown",
            "Order",
            "shop",
        ])
        .prop_map(str::to_owned),
    ]
    .prop_filter("a transition the net holds", |n| {
        !HELD.contains(&n.as_str())
    })
}

fn subject() -> impl Strategy<Value = String> {
    prop_oneof![any::<String>(), Just(String::new()), Just("o1".to_owned())]
}

/// Runs `test` over `strategy` against one netd process; each case calls it
/// on a runtime the test owns, so no case runs inside an async context.
fn against_netd<S: Strategy>(
    strategy: S,
    test: impl AsyncFn(&Cli, S::Value) -> Result<(), TestCaseError>,
) {
    let dir = tempfile::tempdir().unwrap();
    let netd = Process::start(dir.path(), &[&net(dir.path())]);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let cli = rt.block_on(Cli::connect(&netd.url));
    let config = Config {
        cases: 64,
        failure_persistence: None,
        ..Config::default()
    };
    if let Err(e) = TestRunner::new(config).run(&strategy, |v| rt.block_on(test(&cli, v))) {
        panic!("{e}");
    }
}

#[test]
fn why_not_refuses_a_transition_the_net_does_not_hold() {
    against_netd((unheld(), subject()), async |cli, (transition, subject)| {
        let args = json!({"net": "shop", "transition": transition, "subject": subject});
        let refusal = cli.call("why_not", args).await;
        prop_assert!(
            refusal.is_err(),
            "why_not `{transition}` answered: {refusal:?}"
        );
        let refusal = refusal.unwrap_err();
        prop_assert!(
            refusal.contains(&transition),
            "the refusal `{refusal}` names `{transition}`"
        );
        let held = json!({"net": "shop", "transition": "Pay", "subject": "o1"});
        let still = cli.call("why_not", held).await;
        prop_assert!(still.is_ok(), "netd stopped answering: {still:?}");
        Ok(())
    });
}

#[test]
fn why_not_answers_a_held_transition_for_any_subject() {
    let held = prop::sample::select(HELD.to_vec());
    against_netd((held, subject()), async |cli, (transition, subject)| {
        let args = json!({"net": "shop", "transition": transition, "subject": subject});
        let why = cli.call("why_not", args).await;
        prop_assert!(
            why.is_ok(),
            "why_not `{transition}` {subject:?} is refused: {why:?}"
        );
        let why = why.unwrap().structured_content;
        prop_assert!(why.is_some(), "why_not answers without JSON");
        let why = why.unwrap();
        prop_assert_eq!(&why["enabled"], &json!(false), "{}", why);
        prop_assert_eq!(&why["why_not"]["kind"], &json!("needs"), "{}", why);
        Ok(())
    });
}
