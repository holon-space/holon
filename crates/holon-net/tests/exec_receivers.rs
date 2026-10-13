//! Increment 5: receiver kinds resolve an unknown outcome, and a manual fire
//! answers `ACCEPTED(occurrence)` then `settled(occurrence)` (ADR 0032 §9
//! NET-fire.a, NET-manual.a).
//!
//! Twins: `HOLON_NET_TWIN=unknown-is-failure` treats an unknown outcome as a
//! definite failure. `HOLON_NET_TWIN=failed-settles-alone` lets a Failed
//! settle the occurrence while another attempt may have happened.

mod exec_support;

use std::collections::BTreeMap;
use std::time::Duration;

use exec_support::Driver;
use exec_support::content;
use exec_support::int;
use holon_api::Value;
use holon_net::exec::Accepted;
use holon_net::exec::AttemptId;
use holon_net::exec::Body;
use holon_net::exec::Cause;
use holon_net::exec::Class;
use holon_net::exec::EffectResult;
use holon_net::exec::FireError;
use holon_net::exec::NetDef;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Output;
use holon_net::exec::Reader;
use holon_net::exec::Receiver;
use holon_net::exec::Resolution;
use holon_net::exec::Settled;
use holon_net::exec::Subject;
use holon_net::exec::TransitionDef;
use holon_net::exec::Trigger;
use holon_net::exec::UnknownOccurrence;
use proptest::prelude::*;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;
use proptest_state_machine::prop_state_machine;

const MSG: &str = "m";
const KEYED_ATTEMPTS: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    K,
    P,
    N,
}

impl Kind {
    fn transition(self) -> &'static str {
        match self {
            Kind::K => "SendK",
            Kind::P => "SendP",
            Kind::N => "SendN",
        }
    }

    fn place(self) -> &'static str {
        match self {
            Kind::K => "MsgK",
            Kind::P => "MsgP",
            Kind::N => "MsgN",
        }
    }
}

fn n_of(r: &mut Reader<'_>, place: &str, s: &Subject) -> Option<holon_net::exec::Content> {
    Some(content(&[("n", r.field(place, s, "n")?)]))
}

fn net() -> NetDef {
    let send = |kind: Kind, receiver: Receiver| {
        let bind = match kind {
            Kind::K => |r: &mut Reader<'_>, s: &Subject| n_of(r, "MsgK", s),
            Kind::P => |r: &mut Reader<'_>, s: &Subject| n_of(r, "MsgP", s),
            Kind::N => |r: &mut Reader<'_>, s: &Subject| n_of(r, "MsgN", s),
        };
        TransitionDef::new(
            kind.transition(),
            Trigger::Manual,
            bind,
            Body::Effectful(receiver),
        )
        .consumes(kind.place())
    };
    NetDef::default()
        .place("MsgK", Class::Process)
        .place("MsgP", Class::Process)
        .place("MsgN", Class::Process)
        .place("Sent", Class::Process)
        .transition(send(Kind::K, Receiver::Keyed).retry(Duration::from_secs(1), KEYED_ATTEMPTS))
        .transition(send(Kind::P, Receiver::StateProbe))
        .transition(send(Kind::N, Receiver::None))
}

#[derive(Clone, Copy, Debug)]
enum Res {
    Verify,
    Retry,
    Drop,
}

#[derive(Clone, Debug)]
enum Op {
    Write(Kind),
    Fire(Kind),
    Done,
    Failed,
    Unknown,
    Resolve(Res),
}

struct Ref;

impl ReferenceStateMachine for Ref {
    type State = ();
    type Transition = Op;

    fn init_state() -> BoxedStrategy<()> {
        Just(()).boxed()
    }

    fn transitions(_: &()) -> BoxedStrategy<Op> {
        let kind = prop::sample::select(vec![Kind::K, Kind::P, Kind::N]);
        prop_oneof![
            1 => kind.clone().prop_map(Op::Write),
            2 => kind.prop_map(Op::Fire),
            1 => Just(Op::Done),
            1 => Just(Op::Failed),
            2 => Just(Op::Unknown),
            1 => prop::sample::select(vec![Res::Verify, Res::Retry, Res::Drop]).prop_map(Op::Resolve),
        ]
        .boxed()
    }

    fn apply(state: (), _: &Op) {
        state
    }
}

struct Sut {
    d: Driver,
    next_n: i64,
    fires: u64,
    accepted: Vec<OccurrenceId>,
    /// Resolve-Retry count per occurrence.
    retries: BTreeMap<OccurrenceId, usize>,
    /// Definite failures reported per SendN message: each returns the input
    /// and so allows one more run. A stale record of a done send does not.
    refused: BTreeMap<i64, usize>,
    /// The answer the runner gave each attempt, by its result kind.
    answers: BTreeMap<AttemptId, &'static str>,
    dropped: Vec<OccurrenceId>,
}

impl Sut {
    fn answer(&mut self, kind: &'static str) {
        if let Some(req) = self.d.pending.front() {
            self.answers.insert(req.attempt, kind);
        }
    }

    fn oldest_parked(&self) -> Option<OccurrenceId> {
        self.d
            .exec
            .occurrences()
            .find(|(_, s)| matches!(s, Some(Settled::OutcomeUnknown(_))))
            .map(|(o, _)| o.clone())
    }
}

fn sent(req: &holon_net::exec::EffectRequest) -> Vec<Output> {
    vec![Output::new("Sent", &req.subject, req.input.clone())]
}

struct Test;

impl StateMachineTest for Test {
    type SystemUnderTest = Sut;
    type Reference = Ref;

    fn init_test(_: &()) -> Sut {
        Sut {
            d: Driver::new(net()),
            next_n: 0,
            fires: 0,
            accepted: Vec::new(),
            retries: BTreeMap::new(),
            refused: BTreeMap::new(),
            answers: BTreeMap::new(),
            dropped: Vec::new(),
        }
    }

    fn apply(mut sut: Sut, _: &(), op: Op) -> Sut {
        let now = sut.d.now;
        match op {
            Op::Write(kind) => {
                sut.next_n += 1;
                sut.d.exec.write(
                    kind.place(),
                    MSG,
                    content(&[("n", Value::Integer(sut.next_n))]),
                );
                sut.d.step().expect("the net quiesces");
            }
            Op::Fire(kind) => {
                sut.fires += 1;
                let occ = OccurrenceId::caller(&format!("user#{}", sut.fires));
                match sut.d.exec.fire(kind.transition(), MSG, occ.clone(), now) {
                    Ok(Accepted(id)) => {
                        assert_eq!(id, occ, "ACCEPTED names the caller's occurrence");
                        sut.accepted.push(id);
                    }
                    Err(FireError::NotEnabled { .. }) => {}
                    Err(e) => panic!("fire {kind:?}: {e}"),
                }
                sut.d.step().expect("the net quiesces");
            }
            Op::Done => {
                sut.answer("done");
                sut.d
                    .complete_oldest(|req| EffectResult::Done(sent(req)))
                    .expect("the net quiesces")
            }
            Op::Failed => {
                if let Some(req) = sut.d.pending.front()
                    && &*req.transition == "SendN"
                {
                    *sut.refused.entry(int(&req.input, "n")).or_default() += 1;
                }
                sut.answer("failed");
                sut.d
                    .complete_oldest(|_| EffectResult::Failed(Cause::definite("rejected")))
                    .expect("the net quiesces")
            }
            Op::Unknown => {
                sut.answer("unknown");
                sut.d
                    .complete_oldest(|_| EffectResult::Unknown(Cause::definite("connection reset")))
                    .expect("the net quiesces")
            }
            Op::Resolve(res) => {
                if let Some(occ) = sut.oldest_parked() {
                    let resolution = match res {
                        Res::Verify => {
                            let req = sut
                                .d
                                .requests
                                .iter()
                                .find(|r| r.occurrence == occ)
                                .expect("a parked occurrence ran");
                            Resolution::Verify(sent(req))
                        }
                        Res::Retry => {
                            *sut.retries.entry(occ.clone()).or_default() += 1;
                            Resolution::Retry
                        }
                        Res::Drop => {
                            sut.dropped.push(occ.clone());
                            Resolution::Drop
                        }
                    };
                    sut.d
                        .exec
                        .resolve(&occ, resolution, now)
                        .expect("the occurrence is parked");
                    sut.d.step().expect("the net quiesces");
                }
            }
        }
        sut
    }

    fn check_invariants(sut: &Sut, _: &()) {
        let mut per_n: BTreeMap<i64, usize> = BTreeMap::new();
        let mut per_occ: BTreeMap<&OccurrenceId, usize> = BTreeMap::new();
        for req in &sut.d.requests {
            if !req.probe {
                *per_occ.entry(&req.occurrence).or_default() += 1;
            }
            if &*req.transition == "SendN" {
                *per_n.entry(int(&req.input, "n")).or_default() += 1;
            }
        }
        let retries_of = |pred: &dyn Fn(&OccurrenceId) -> bool| -> usize {
            sut.retries
                .iter()
                .filter(|(o, _)| pred(o))
                .map(|(_, c)| c)
                .sum()
        };
        for (n, runs) in per_n {
            let allowed = 1
                + sut.refused.get(&n).copied().unwrap_or(0)
                + retries_of(&|o| {
                    sut.d.requests.iter().any(|r| {
                        &r.occurrence == o && &*r.transition == "SendN" && int(&r.input, "n") == n
                    })
                });
            assert!(
                runs <= allowed,
                "none_effect_runs_once: SendN ran {runs} times for message {n}, allowed {allowed}"
            );
        }
        for (occ, runs) in per_occ {
            let allowed = KEYED_ATTEMPTS as usize + retries_of(&|o| o == occ);
            assert!(
                runs <= allowed,
                "attempts_per_occurrence: {occ} ran {runs} times, allowed {allowed}"
            );
        }
        for occ in &sut.accepted {
            sut.d
                .exec
                .settled(occ)
                .expect("settled answers for every accepted occurrence");
        }
        // A probe reads the goal for every attempt, so SendP is left out.
        for (occ, settled) in sut.d.exec.occurrences() {
            let runs: Vec<_> = sut
                .d
                .requests
                .iter()
                .filter(|r| &r.occurrence == occ && &*r.transition != "SendP")
                .map(|r| sut.answers.get(&r.attempt).copied())
                .collect();
            if runs.is_empty() || sut.dropped.contains(occ) {
                continue;
            }
            if matches!(settled, Some(Settled::Failed(_))) {
                assert!(
                    runs.iter().all(|a| *a == Some("failed")),
                    "failed_only_when_every_attempt_failed: {occ} settled {settled:?} over {runs:?}"
                );
            }
            if runs.contains(&Some("done")) {
                assert!(
                    matches!(
                        settled,
                        Some(Settled::Done | Settled::DoneOverChangedInputs(_))
                    ),
                    "done_when_any_attempt_done: {occ} settled {settled:?}"
                );
            }
        }
        let never = OccurrenceId::caller("never-fired");
        assert_eq!(
            sut.d.exec.settled(&never),
            Err(UnknownOccurrence(never.clone())),
            "settled of an unknown id is an error"
        );
    }
}

prop_state_machine! {
    #[test]
    fn unknown_outcomes_never_run_an_effect_twice(sequential 1..40 => Test);
}

/// A done effect is a fact (NET-fire.a): over stale inputs its input is still
/// consumed, so a manual re-fire has nothing to send twice.
#[test]
fn a_stale_done_send_is_a_fact() {
    let (mut d, first) = send_over_changed_config();
    d.complete_oldest(|req| EffectResult::Done(sent(req)))
        .expect("the net quiesces");

    let again = d
        .exec
        .fire("Send", MSG, OccurrenceId::caller("send#2"), d.now);
    d.step().expect("the net quiesces");
    assert_eq!(
        d.requests_of("Send").count(),
        1,
        "stale_done_never_sends_twice: the re-fire sent message 1 again ({again:?})"
    );
    assert!(
        matches!(
            d.exec.settled(&first),
            Ok(Some(Settled::DoneOverChangedInputs(_)))
        ),
        "stale_done_is_disclosed: send#1 settled {:?}",
        d.exec.settled(&first)
    );
    assert!(
        d.exec.tokens().token("Sent", MSG).is_none(),
        "stale_done_withholds_outputs: Sent holds the stale output"
    );
}

/// A manual `Send` that consumes `Msg` and reads `Config`, fired, with
/// `Config` changed while the effect runs.
fn send_over_changed_config() -> (Driver, OccurrenceId) {
    let net = NetDef::default()
        .place("Msg", Class::Process)
        .place("Config", Class::Process)
        .place("Sent", Class::Process)
        .transition(
            TransitionDef::new(
                "Send",
                Trigger::Manual,
                |r, s| n_of(r, "Msg", s),
                Body::Effectful(Receiver::None),
            )
            .consumes("Msg")
            .reads("Config"),
        );
    let mut d = Driver::new(net);
    let n = |v: i64| content(&[("n", Value::Integer(v))]);
    d.exec.write("Msg", MSG, n(1));
    d.exec.write("Config", MSG, n(1));
    d.step().expect("the net quiesces");
    let first = OccurrenceId::caller("send#1");
    d.exec
        .fire("Send", MSG, first.clone(), d.now)
        .expect("Msg holds a message");
    d.step().expect("the net quiesces");
    d.exec.write("Config", MSG, n(2));
    (d, first)
}

/// A stale record withholds nothing when the effect has no outputs, but the
/// changed inputs are still disclosed.
#[test]
fn a_stale_done_without_outputs_is_disclosed() {
    let (mut d, first) = send_over_changed_config();
    d.complete_oldest(|_| EffectResult::Done(Vec::new()))
        .expect("the net quiesces");
    assert!(
        matches!(
            d.exec.settled(&first),
            Ok(Some(Settled::DoneOverChangedInputs(_)))
        ),
        "stale_done_is_disclosed: send#1 without outputs settled {:?}",
        d.exec.settled(&first)
    );
}

/// A caller-supplied occurrence id never collides with a minted one, and a
/// duplicate caller id is an error to the caller (NET-manual.a).
#[test]
fn caller_and_minted_occurrence_ids_never_collide() {
    let net = NetDef::default()
        .place("In", Class::Process)
        .place("Seen", Class::Process)
        .place("Out", Class::Process)
        .transition(
            TransitionDef::new(
                "M",
                Trigger::Manual,
                |r, s| n_of(r, "In", s),
                Body::Pure(|s, input| Ok(vec![Output::new("Seen", s, input.clone())])),
            )
            .reads("In"),
        )
        .transition(
            TransitionDef::new(
                "A",
                Trigger::Auto,
                |r, s| n_of(r, "In", s),
                Body::Pure(|s, input| Ok(vec![Output::new("Out", s, input.clone())])),
            )
            .consumes("In"),
        );
    let mut d = Driver::new(net);
    let n = |v: i64| content(&[("n", Value::Integer(v))]);
    d.exec.write("In", MSG, n(1));
    let manual = OccurrenceId::caller("A#1");
    d.exec
        .fire("M", MSG, manual.clone(), d.now)
        .expect("In holds a token");
    d.step().expect("the net quiesces");
    assert_eq!(d.exec.settled(&manual), Ok(Some(Settled::Done)));
    assert_eq!(
        d.exec.occurrences().count(),
        2,
        "the manual and the minted occurrence are distinct"
    );
    assert!(
        d.exec.tokens().token("Out", MSG).is_some(),
        "the auto firing of A recorded"
    );

    d.exec.write("In", MSG, n(2));
    assert_eq!(
        d.exec.fire("M", MSG, manual.clone(), d.now),
        Err(FireError::DuplicateOccurrence(manual)),
        "a duplicate caller id is an error"
    );
}
