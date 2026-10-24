//! Probes against a controlled clock: their stdout becomes fact tokens, a
//! failed run keeps the facts it cannot refresh and discloses them as stale,
//! and the next good run clears that condition.

use std::sync::Arc;
use std::time::Duration;

use holon_net::exec::Content;
use holon_net::exec::Executor;
use holon_net::exec::Reader;
use holon_net::exec::Subject;
use holon_net::file::NetFile;
use holon_net::load::Logic;
use holon_net::load::net_def;
use holon_net::probe::ProbeOutcome;
use holon_net::probe::Probes;
use holon_pattern::Value;

const NET: &str = "\
name: probed
places:
  Process:
    capacity: register
transitions:
  Watch:
    trigger: manual
    priority: 0
    reads:
    - Process
probes:
  live_pids:
    command:
      argv:
      - ps
      working_dir: .
      env_allow: []
      timeout: 1s
    interval: 10s
    facts_into: Process
    subject_field: pid
";

const INTERVAL: Duration = Duration::from_secs(10);

fn at(secs: u64) -> Duration {
    Duration::from_secs(secs)
}

fn setup() -> (Executor, Probes) {
    let file = NetFile::parse(NET).expect("the test net parses");
    let net = net_def("probed.yaml", NET, |_| {
        Logic::pure(
            Arc::new(|_: &mut Reader<'_>, _: &Subject| None),
            Arc::new(|_: &Subject, _: &Content| Ok(vec![])),
        )
    })
    .expect("the test net loads");
    let exec = Executor::new(net).expect("the test net checks");
    (exec, Probes::new(&file.probes, at(0)))
}

fn content(pairs: &[(&str, Value)]) -> Content {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

fn subjects(exec: &Executor) -> Vec<String> {
    exec.tokens()
        .subjects("Process")
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Starts the one probe, which must be due at `now`, and settles it.
fn run(exec: &mut Executor, probes: &mut Probes, outcome: ProbeOutcome, now: Duration) {
    let due = probes.due(now);
    assert_eq!(
        due.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        ["live_pids"],
        "the probe is due at {now:?}"
    );
    let requests = probes
        .settle(exec, "live_pids", outcome, now)
        .expect("the net quiesces");
    assert_eq!(requests, vec![], "the test net runs no effect");
}

fn good(stdout: &str) -> ProbeOutcome {
    ProbeOutcome::Output(stdout.to_owned())
}

#[test]
fn each_stdout_line_becomes_a_fact_token_keyed_by_its_subject_field() {
    let (mut exec, mut probes) = setup();
    run(
        &mut exec,
        &mut probes,
        good("{\"pid\": 41, \"command\": \"cargo\"}\n{\"pid\": \"42\", \"command\": \"bash\"}\n"),
        at(0),
    );
    assert_eq!(subjects(&exec), ["41", "42"]);
    assert_eq!(
        exec.tokens().token("Process", "41").map(|t| &t.content),
        Some(&content(&[
            ("pid", Value::Integer(41)),
            ("command", Value::String("cargo".to_owned())),
        ]))
    );
    assert_eq!(probes.conditions(), vec![]);
}

#[test]
fn a_failing_run_keeps_the_facts_marks_them_stale_and_the_next_good_run_clears_it() {
    let (mut exec, mut probes) = setup();
    run(
        &mut exec,
        &mut probes,
        good("{\"pid\": 41}\n{\"pid\": 42}\n"),
        at(0),
    );
    let version = exec.tokens().version("Process", "41");

    run(
        &mut exec,
        &mut probes,
        ProbeOutcome::Failed("exited 1: ps: no such process table".to_owned()),
        INTERVAL,
    );
    assert_eq!(subjects(&exec), ["41", "42"], "a failure deletes no fact");
    assert_eq!(
        exec.tokens().version("Process", "41"),
        version,
        "a failure does not touch the facts it keeps"
    );
    let conditions = probes.conditions();
    assert_eq!(conditions.len(), 1, "one failing probe: {conditions:?}");
    let condition = &conditions[0];
    assert_eq!(condition.probe, "live_pids");
    assert_eq!(condition.place.as_str(), "Process");
    assert_eq!(condition.since, INTERVAL);
    assert_eq!(
        condition.stale,
        vec![Subject::new("41"), Subject::new("42")]
    );
    let text = condition.to_string();
    assert!(
        text.contains("live_pids") && text.contains("exited 1") && text.contains("stale"),
        "the condition names the probe, the cause and the stale facts: {text}"
    );

    run(
        &mut exec,
        &mut probes,
        good("{\"pid\": 42}\n"),
        INTERVAL * 2,
    );
    assert_eq!(
        probes.conditions(),
        vec![],
        "a good run clears the condition"
    );
    assert_eq!(
        subjects(&exec),
        ["42"],
        "a good run is the whole truth: a subject it no longer reports goes"
    );
}

#[test]
fn a_run_whose_stdout_is_not_facts_is_a_failure_and_writes_nothing() {
    let (mut exec, mut probes) = setup();
    run(&mut exec, &mut probes, good("{\"pid\": 41}\n"), at(0));
    let version = exec.tokens().version("Process", "41");

    for (i, stdout) in [
        "{\"pid\": 42}\nnot json\n",
        "{\"pid\": 42}\n[1, 2]\n",
        "{\"pid\": 42}\n{\"command\": \"no subject\"}\n",
        "{\"pid\": 42}\n{\"pid\": true}\n",
        "{\"pid\": 42}\n{\"pid\": 42}\n",
    ]
    .into_iter()
    .enumerate()
    {
        let now = INTERVAL * (i as u32 + 1);
        run(&mut exec, &mut probes, good(stdout), now);
        assert_eq!(subjects(&exec), ["41"], "no partial write for {stdout:?}");
        assert_eq!(exec.tokens().version("Process", "41"), version);
        let conditions = probes.conditions();
        assert_eq!(conditions.len(), 1, "{stdout:?} raises the condition");
        assert!(
            conditions[0].cause.contains("line 2"),
            "the cause names the refused line: {}",
            conditions[0].cause
        );
    }
}

#[test]
fn a_probe_runs_once_at_start_and_then_one_interval_after_each_settle() {
    let (mut exec, mut probes) = setup();
    let due = probes.due(at(0));
    assert_eq!(due.len(), 1, "every probe is due when the host starts");
    assert_eq!(due[0].1.argv, ["ps"], "the host gets the command to run");
    assert_eq!(
        probes.due(at(30)),
        vec![],
        "a running probe is not due again"
    );

    probes
        .settle(&mut exec, "live_pids", good(""), at(3))
        .expect("the net quiesces");
    assert_eq!(
        probes.due(at(12)),
        vec![],
        "the interval counts from the settle"
    );
    assert_eq!(probes.due(at(13)).len(), 1);
}
