//! A subject a good run stops reporting while a parked firing holds its token.

use std::sync::Arc;
use std::time::Duration;

use holon_net::exec::Body;
use holon_net::exec::Cause;
use holon_net::exec::Class;
use holon_net::exec::Content;
use holon_net::exec::EffectResult;
use holon_net::exec::Executor;
use holon_net::exec::NetDef;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Read;
use holon_net::exec::Receiver;
use holon_net::exec::TransitionDef;
use holon_net::exec::Trigger;
use holon_net::file::NetFile;
use holon_net::probe::ProbeOutcome;
use holon_net::probe::Probes;

const FILE: &str = "\
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

fn subjects(exec: &Executor) -> Vec<String> {
    exec.tokens()
        .subjects("Process")
        .iter()
        .map(|s| s.to_string())
        .collect()
}

#[test]
fn a_subject_dropped_while_a_parked_firing_holds_it_is_retracted_by_a_later_good_run() {
    let always: Read = Arc::new(|_, _| Some(Content::new()));
    let net = NetDef::default()
        .register("Process", Class::Process)
        .transition(
            TransitionDef::new(
                "Use",
                Trigger::Manual,
                always,
                Body::Effectful(Receiver::None),
            )
            .consumes("Process")
            .abandon(Duration::from_secs(3)),
        );
    let mut exec = Executor::new(net).unwrap();
    let file = NetFile::parse(FILE).unwrap();
    let mut probes = Probes::new(&file.probes, Duration::ZERO);
    let at = Duration::from_secs;
    let mut tick = |exec: &mut Executor, out: &str, now: Duration| {
        assert_eq!(probes.due(now).len(), 1);
        probes
            .settle(exec, "live_pids", ProbeOutcome::Output(out.to_owned()), now)
            .unwrap();
    };

    tick(&mut exec, "{\"pid\": 41}\n{\"pid\": 42}\n", at(0));
    let q = exec
        .fire(
            "Use",
            "41",
            OccurrenceId::caller("use#1"),
            Content::new(),
            at(1),
        )
        .unwrap();
    let attempt = q.requests[0].attempt;
    exec.step(at(5)).unwrap();
    assert!(
        exec.in_flight().next().is_none(),
        "the abandoned firing is parked"
    );
    tick(&mut exec, "{\"pid\": 42}\n", at(10));
    drop(
        exec.complete(attempt, EffectResult::Failed(Cause::definite("no")), at(11))
            .unwrap(),
    );
    assert_eq!(
        subjects(&exec),
        ["41", "42"],
        "the late failure of the parked firing gave 41 back"
    );
    tick(&mut exec, "{\"pid\": 42}\n", at(20));
    assert_eq!(
        subjects(&exec),
        ["42"],
        "a good run is the whole truth: 41, reported by no run since, stays"
    );
}
