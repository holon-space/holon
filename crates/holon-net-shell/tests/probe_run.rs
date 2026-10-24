//! Probes against real children: fake probe scripts feed facts, fail and
//! recover, and the orchestration probes of the work-runtime net report
//! facts about this machine.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use holon_net::exec::Content;
use holon_net::exec::Executor;
use holon_net::exec::Reader;
use holon_net::exec::Subject;
use holon_net::file::CommandSpec;
use holon_net::file::NetFile;
use holon_net::load::Logic;
use holon_net::load::net_def;
use holon_net::probe::ProbeOutcome;
use holon_net::probe::Probes;
use holon_net_shell::probe::run;

/// A net whose transitions never bind: the probes alone change its marking.
fn load(path: &str, text: &str) -> (NetFile, Executor) {
    let file = NetFile::parse(text).expect("the net parses");
    let inert = |name: &str| {
        let bind = Arc::new(|_: &mut Reader<'_>, _: &Subject| None);
        match file.transitions[name].effect {
            Some(_) => Logic::effect(bind),
            None => Logic::pure(bind, Arc::new(|_: &Subject, _: &Content| Ok(vec![]))),
        }
    };
    let exec =
        Executor::new(net_def(path, text, inert).expect("the net loads")).expect("the net checks");
    (file, exec)
}

fn command(argv: &[&str], timeout: &str, working_dir: &Path) -> CommandSpec {
    serde_json::from_value(serde_json::json!({
        "argv": argv,
        "working_dir": working_dir.to_str().expect("the temporary path is utf-8"),
        "env_allow": ["PATH"],
        "timeout": timeout,
    }))
    .expect("the command is valid")
}

fn subjects(exec: &Executor, place: &str) -> Vec<String> {
    exec.tokens()
        .subjects(place)
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Runs every probe due at `now` and settles it.
async fn tick(exec: &mut Executor, probes: &mut Probes, now: Duration) {
    for (name, command) in probes.due(now) {
        let outcome = run(&command).await;
        let requests = probes
            .settle(exec, &name, outcome, now)
            .expect("the net quiesces");
        assert_eq!(requests, vec![], "the test net runs no effect");
    }
}

/// `fake.sh` answers as the file `mode` says: two agents, one agent, or a
/// failure.
const FAKE: &str = r#"#!/usr/bin/env bash
set -euo pipefail
case "$(cat mode)" in
  two) echo '{"agent": "a1", "alive": true}'; echo '{"agent": "a2", "alive": true}' ;;
  one) echo '{"agent": "a2", "alive": false}' ;;
  fail) echo "registry unreadable" >&2; exit 3 ;;
esac
"#;

fn fake_net(dir: &Path) -> String {
    format!(
        "\
name: fake
places:
  Agent:
    capacity: register
transitions:
  Watch:
    trigger: manual
    priority: 0
    reads:
    - Agent
probes:
  agents:
    command:
      argv:
      - bash
      - fake.sh
      working_dir: {}
      env_allow:
      - PATH
      timeout: 5s
    interval: 1m
    facts_into: Agent
    subject_field: agent
",
        dir.display()
    )
}

#[tokio::test]
async fn a_fake_probe_feeds_facts_a_failure_shows_a_condition_and_a_good_run_clears_it() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    fs::write(dir.path().join("fake.sh"), FAKE).expect("the fake probe is written");
    let mode = |m: &str| fs::write(dir.path().join("mode"), m).expect("the mode is written");
    let text = fake_net(dir.path());
    let (file, mut exec) = load("fake.yaml", &text);
    let mut probes = Probes::new(&file.probes, Duration::ZERO);
    let minute = Duration::from_secs(60);

    mode("two");
    tick(&mut exec, &mut probes, Duration::ZERO).await;
    assert_eq!(subjects(&exec, "Agent"), ["a1", "a2"], "the facts appear");
    assert_eq!(probes.conditions(), vec![]);

    mode("fail");
    tick(&mut exec, &mut probes, minute).await;
    assert_eq!(
        subjects(&exec, "Agent"),
        ["a1", "a2"],
        "a failure keeps them"
    );
    let conditions = probes.conditions();
    assert_eq!(
        conditions.len(),
        1,
        "the failing probe shows: {conditions:?}"
    );
    assert!(
        conditions[0].cause.contains("registry unreadable"),
        "the condition carries the probe's own words: {}",
        conditions[0]
    );
    assert_eq!(
        conditions[0].stale,
        vec![Subject::new("a1"), Subject::new("a2")]
    );

    mode("one");
    tick(&mut exec, &mut probes, minute * 2).await;
    assert_eq!(probes.conditions(), vec![], "the next good run clears it");
    assert_eq!(subjects(&exec, "Agent"), ["a2"]);
}

#[tokio::test]
async fn a_probe_past_its_timeout_fails_and_its_process_group_is_ended() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let script = "sleep 30 & echo $! > child.pid; wait";
    let outcome = run(&command(&["bash", "-c", script], "300ms", dir.path())).await;
    let ProbeOutcome::Failed(cause) = outcome else {
        panic!("a probe past its timeout fails: {outcome:?}");
    };
    assert!(cause.contains("timed out after 300ms"), "{cause}");
    let child: i32 = fs::read_to_string(dir.path().join("child.pid"))
        .expect("the probe wrote its child's pid")
        .trim()
        .parse()
        .expect("a pid");
    // The orphaned child is reaped by init, a moment after the group ends.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while unsafe { libc::kill(child, 0) } == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the probe's background child {child} never ended with its process group"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_probe_that_cannot_start_fails_with_the_reason() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let outcome = run(&command(&["./no-such-probe"], "1s", dir.path())).await;
    let ProbeOutcome::Failed(cause) = outcome else {
        panic!("a probe that cannot start fails: {outcome:?}");
    };
    assert!(cause.contains("cannot start ./no-such-probe"), "{cause}");
}

/// The work-runtime net's own probes, run from this checkout. The registry
/// probe reads a registry the test writes, not the machine's.
#[tokio::test]
async fn the_orchestration_probes_report_this_machine() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let text = fs::read_to_string(root.join("crates/holon-net/nets/work-runtime.yaml"))
        .expect("the work-runtime net is readable");
    let (file, mut exec) = load("work-runtime.yaml", &text);
    let registry = tempfile::tempdir().expect("a temporary directory");
    let agents = registry.path().join("some-repo/agents");
    fs::create_dir_all(&agents).expect("the registry is created");
    fs::write(
        agents.join("a1.json"),
        r#"{"v":1,"agentId":"a1","status":"running"}"#,
    )
    .expect("the agent record is written");
    fs::create_dir_all(registry.path().join("some-repo/jobs")).expect("the jobs dir");
    let mut probes = file.probes.clone();
    probes.shift_remove("board_cards");
    for spec in probes.values_mut() {
        spec.command.working_dir = root.display().to_string();
        if spec.command.argv.iter().any(|a| a.ends_with("registry.sh")) {
            spec.command
                .argv
                .push(registry.path().display().to_string());
        }
    }
    let mut probes = Probes::new(&probes, Duration::ZERO);
    tick(&mut exec, &mut probes, Duration::ZERO).await;

    assert_eq!(probes.conditions(), vec![], "every probe ran clean");
    assert_eq!(subjects(&exec, "Agent"), ["a1"]);
    assert_eq!(subjects(&exec, "JobRecord"), Vec::<String>::new());
    let me = std::process::id().to_string();
    assert!(
        subjects(&exec, "Process").contains(&me),
        "the pid probe sees this test process {me}"
    );
    let disks = subjects(&exec, "Disk");
    assert_eq!(disks.len(), 1, "one volume holds the checkout: {disks:?}");
    let disk = &exec
        .tokens()
        .token("Disk", &disks[0])
        .expect("a fact")
        .content;
    assert!(
        matches!(disk.get("free_kb"), Some(holon_pattern::Value::Integer(kb)) if *kb > 0),
        "the disk fact names its free space: {disk:?}"
    );
    assert_eq!(subjects(&exec, "Host").len(), 1, "one fact for this host");
}
