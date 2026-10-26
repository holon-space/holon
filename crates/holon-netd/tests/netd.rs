//! netd as a process: it starts, takes puts and fires over its MCP endpoint,
//! answers awaits, refuses by name, and after a kill rebuilds its marking
//! from its probes alone.

mod common;

use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use common::Cli;
use common::Process;
use serde_json::Value;
use serde_json::json;

/// `Pay` is pure, `Ship` runs `true`, `Jam` runs `false` into its failure
/// port, and the probe reports the lines of `seen.jsonl`.
fn orders(dir: &Path) -> PathBuf {
    let dir = dir.display();
    let text = format!(
        r#"name: orders
places:
  Order:
    capacity: single
  Paid:
    capacity: single
  Shipped:
    capacity: single
  Jammed:
    capacity: single
  Seen:
    capacity: register
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
  Jam:
    trigger: manual
    priority: 1
    consumes:
    - Paid
    on_failure: Jammed
    effect: jam
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
    log_path: {dir}/logs/{{transition}}-{{subject}}-{{attempt}}.log
    exit_codes:
      done:
      - 0
      failed: []
      other: outcome_unknown
  jam:
    kind: shell
    command:
      argv:
      - /usr/bin/false
      working_dir: {dir}
      env_allow: []
      timeout: 10s
    log_path: {dir}/logs/{{transition}}-{{subject}}-{{attempt}}.log
    exit_codes:
      done: []
      failed:
      - 1
      other: outcome_unknown
probes:
  seen:
    command:
      argv:
      - /bin/cat
      - seen.jsonl
      working_dir: {dir}
      env_allow: []
      timeout: 10s
    interval: 1h
    facts_into: Seen
    subject_field: id
"#
    );
    fs::write(format!("{dir}/seen.jsonl"), "").expect("the probe file is writable");
    let path = PathBuf::from(format!("{dir}/orders.yaml"));
    fs::write(&path, text).expect("the net file is writable");
    path
}

fn report(dir: &Path, ids: &[&str]) {
    let lines: String = ids
        .iter()
        .map(|id| format!("{{\"id\":\"{id}\"}}\n"))
        .collect();
    fs::write(dir.join("seen.jsonl"), lines).expect("the probe file is writable");
}

/// The subjects of `place` in a marking.
fn subjects(marking: &Value, place: &str) -> Vec<String> {
    marking["places"][place]
        .as_object()
        .unwrap_or_else(|| panic!("the marking has the place {place}: {marking}"))
        .keys()
        .cloned()
        .collect()
}

async fn settle(cli: &Cli, occurrence: &str) -> Value {
    let call = cli.json("await", json!({"net": "orders", "occurrence": occurrence}));
    tokio::time::timeout(Duration::from_secs(20), call)
        .await
        .unwrap_or_else(|_| panic!("the occurrence {occurrence} never settles"))
}

#[tokio::test]
async fn it_starts_with_the_facts_its_probes_report() {
    let dir = tempfile::tempdir().unwrap();
    let net = orders(dir.path());
    report(dir.path(), &["a", "b"]);
    let netd = Process::start(dir.path(), &[&net]);
    let cli = Cli::connect(&netd.url).await;
    let marking = cli.json("marking", json!({"net": "orders"})).await;
    assert_eq!(subjects(&marking, "Seen"), ["a", "b"]);
    assert_eq!(subjects(&marking, "Order"), Vec::<String>::new());
}

#[tokio::test]
async fn a_put_fact_shows_in_the_marking_as_json_and_text() {
    let dir = tempfile::tempdir().unwrap();
    let netd = Process::start(dir.path(), &[&orders(dir.path())]);
    let cli = Cli::connect(&netd.url).await;
    cli.json(
        "put",
        json!({"net": "orders", "place": "Order", "subject": "o1", "content": {"qty": 2}}),
    )
    .await;
    let marking = cli.json("marking", json!({"net": "orders"})).await;
    assert_eq!(marking["places"]["Order"]["o1"], json!([{"qty": 2}]));
    let text = cli.text("marking", json!({"net": "orders"})).await;
    assert!(
        text.lines().any(|l| l == r#"Order o1 {"qty":2}"#),
        "the text marking has one line per token: {text}"
    );
}

#[tokio::test]
async fn a_pure_fire_settles_done_and_moves_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let netd = Process::start(dir.path(), &[&orders(dir.path())]);
    let cli = Cli::connect(&netd.url).await;
    let put = json!({"net": "orders", "place": "Order", "subject": "o1", "content": {}});
    cli.json("put", put).await;
    let fire =
        json!({"net": "orders", "transition": "Pay", "subject": "o1", "occurrence": "pay-1"});
    let accepted = cli.json("fire", fire).await;
    assert_eq!(accepted["occurrence"], "pay-1");
    assert_eq!(settle(&cli, "pay-1").await["settled"], "done");
    let marking = cli.json("marking", json!({"net": "orders"})).await;
    assert_eq!(subjects(&marking, "Order"), Vec::<String>::new());
    assert_eq!(subjects(&marking, "Paid"), ["o1"]);
}

#[tokio::test]
async fn an_effect_fire_settles_by_the_exit_code_of_its_command() {
    let dir = tempfile::tempdir().unwrap();
    let netd = Process::start(dir.path(), &[&orders(dir.path())]);
    let cli = Cli::connect(&netd.url).await;
    for subject in ["o1", "o2"] {
        let put = json!({"net": "orders", "place": "Paid", "subject": subject, "content": {}});
        cli.json("put", put).await;
    }
    let ship =
        json!({"net": "orders", "transition": "Ship", "subject": "o1", "occurrence": "ship-1"});
    cli.json("fire", ship).await;
    let jam = json!({"net": "orders", "transition": "Jam", "subject": "o2", "occurrence": "jam-1"});
    cli.json("fire", jam).await;
    assert_eq!(settle(&cli, "ship-1").await["settled"], "done");
    let jammed = settle(&cli, "jam-1").await;
    assert_eq!(jammed["settled"], "failed", "{jammed}");
    let marking = cli.json("marking", json!({"net": "orders"})).await;
    assert_eq!(subjects(&marking, "Shipped"), ["o1"]);
    assert_eq!(subjects(&marking, "Jammed"), ["o2"]);
    let text = cli
        .text("await", json!({"net": "orders", "occurrence": "jam-1"}))
        .await;
    assert!(text.starts_with("failed: "), "{text}");
}

#[tokio::test]
async fn why_not_names_the_missing_input() {
    let dir = tempfile::tempdir().unwrap();
    let netd = Process::start(dir.path(), &[&orders(dir.path())]);
    let cli = Cli::connect(&netd.url).await;
    let ask = json!({"net": "orders", "transition": "Pay", "subject": "o1"});
    let why = cli.json("why_not", ask.clone()).await;
    assert_eq!(why["enabled"], false);
    assert_eq!(why["why_not"]["kind"], "needs");
    assert_eq!(why["why_not"]["place"], "Order");
    let put = json!({"net": "orders", "place": "Order", "subject": "o1", "content": {}});
    cli.json("put", put).await;
    assert_eq!(cli.json("why_not", ask.clone()).await["enabled"], true);
    assert_eq!(cli.text("why_not", ask).await, "Pay o1: enabled");
}

#[tokio::test]
async fn every_refusal_names_what_it_refused() {
    let dir = tempfile::tempdir().unwrap();
    let netd = Process::start(dir.path(), &[&orders(dir.path())]);
    let cli = Cli::connect(&netd.url).await;
    let cases = [
        (
            "fire",
            json!({"net": "orders", "transition": "Teleport", "subject": "o1", "occurrence": "x"}),
            "Teleport",
        ),
        (
            "fire",
            json!({"net": "orders", "transition": "Pay", "subject": "o9", "occurrence": "x"}),
            "o9",
        ),
        (
            "put",
            json!({"net": "orders", "place": "Nowhere", "subject": "o1", "content": {}}),
            "Nowhere",
        ),
        (
            "put",
            json!({"net": "orders", "place": "Order", "subject": "o1", "content": [1, 2]}),
            "not a JSON object",
        ),
        ("marking", json!({"net": "invoices"}), "invoices"),
        (
            "await",
            json!({"net": "orders", "occurrence": "never-fired"}),
            "never-fired",
        ),
        (
            "why_not",
            json!({"net": "orders", "transition": "Teleport", "subject": "o1"}),
            "Teleport",
        ),
    ];
    for (tool, args, named) in cases {
        let refusal = cli.refusal(tool, args.clone()).await;
        assert!(
            refusal.contains(named),
            "{tool} {args}: the refusal `{refusal}` names `{named}`"
        );
    }
}

#[tokio::test]
async fn a_second_netd_on_the_same_port_is_refused_by_address() {
    let dir = tempfile::tempdir().unwrap();
    let net = orders(dir.path());
    let first = Process::start(dir.path(), &[&net]);
    let addr = first
        .url
        .trim_start_matches("http://")
        .trim_end_matches("/mcp")
        .to_owned();
    let other = tempfile::tempdir().unwrap();
    let net2 = orders(other.path());
    let refused = Process::start_on(&addr, other.path(), &[&net2])
        .err()
        .expect("the port is taken");
    assert!(refused.contains(&addr), "{refused}");
}

#[tokio::test]
async fn a_broken_net_file_is_refused_by_path() {
    let dir = tempfile::tempdir().unwrap();
    let net = orders(dir.path());
    let text = fs::read_to_string(&net).unwrap();
    fs::write(
        &net,
        text.replace("    - Order\n    produces:", "    - Ordr\n    produces:"),
    )
    .unwrap();
    let refused = Process::start_on("127.0.0.1:0", dir.path(), &[&net])
        .err()
        .expect("the net file is broken");
    assert!(
        refused.contains(&net.display().to_string()) && refused.contains("Ordr"),
        "{refused}"
    );
}

#[tokio::test]
async fn a_listen_address_off_the_loopback_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let net = orders(dir.path());
    let refused = Process::start_on("0.0.0.0:0", dir.path(), &[&net])
        .err()
        .expect("netd listens on the loopback only");
    assert!(refused.contains("0.0.0.0:0"), "{refused}");
}

#[tokio::test]
async fn after_a_kill_the_marking_is_what_the_probes_report_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let net = orders(dir.path());
    report(dir.path(), &["a", "b"]);
    let old = Process::start(dir.path(), &[&net]);
    let cli = Cli::connect(&old.url).await;
    let put = json!({"net": "orders", "place": "Order", "subject": "only-old", "content": {}});
    cli.json("put", put).await;
    let put = json!({"net": "orders", "place": "Seen", "subject": "put-by-hand", "content": {}});
    cli.json("put", put).await;
    let before = cli.json("marking", json!({"net": "orders"})).await;
    assert_eq!(subjects(&before, "Order"), ["only-old"]);
    drop(cli);
    report(dir.path(), &["b", "c"]);
    old.kill();

    let new = Process::start(dir.path(), &[&net]);
    let cli = Cli::connect(&new.url).await;
    let after = cli.json("marking", json!({"net": "orders"})).await;
    let places = after["places"].as_object().expect("places is a map");
    for (place, tokens) in places {
        let expected = if place == "Seen" {
            json!({"b": [{"id": "b"}], "c": [{"id": "c"}]})
        } else {
            json!({})
        };
        assert_eq!(tokens, &expected, "the place {place} after the restart");
    }
}

#[tokio::test]
async fn an_effect_its_command_timeout_kills_still_settles() {
    let dir = tempfile::tempdir().unwrap();
    let shown = dir.path().display();
    let text = format!(
        r#"name: slow
places:
  In:
    capacity: single
  Out:
    capacity: single
transitions:
  Hang:
    trigger: manual
    priority: 1
    consumes:
    - In
    produces:
    - Out
    effect: hang
    receiver: none
effects:
  hang:
    kind: shell
    command:
      argv:
      - /bin/sleep
      - "30"
      working_dir: {shown}
      env_allow: []
      timeout: 1s
    log_path: {shown}/logs/{{transition}}-{{subject}}-{{attempt}}.log
    exit_codes:
      done:
      - 0
      failed: []
      other: outcome_unknown
"#
    );
    let net = dir.path().join("slow.yaml");
    fs::write(&net, text).unwrap();
    let netd = Process::start(dir.path(), &[&net]);
    let cli = Cli::connect(&netd.url).await;
    let put = json!({"net": "slow", "place": "In", "subject": "h", "content": {}});
    cli.json("put", put).await;
    let fire = json!({"net": "slow", "transition": "Hang", "subject": "h", "occurrence": "hang-1"});
    cli.json("fire", fire).await;
    let call = cli.json("await", json!({"net": "slow", "occurrence": "hang-1"}));
    let settled = tokio::time::timeout(Duration::from_secs(15), call)
        .await
        .expect("a run its 1s command timeout killed settles within 15s");
    assert_ne!(settled["settled"], "done", "{settled}");
}
