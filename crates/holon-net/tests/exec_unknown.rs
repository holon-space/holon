//! An unknown outcome is not a failure (ADR 0032 §9 NET-fire.a): while the
//! effect may have happened, the next producer stays blocked. Only a definite
//! failure, Drop or the run timer lets it fire, and a later Verify of the
//! first producer is a stale record of a done effect: a candidate or a
//! disclosed condition, never a second product and never a silent discard.
//!
//! A result closes only its own attempt; the occurrence outcome is derived
//! from all its attempts: Done when any is Done, Failed only when every one
//! is Failed, else unknown and parked.
//!
//! Twins: `HOLON_NET_TWIN=unknown-settles-failed` settles the run failed when
//! the outcome parks, which puts the failure port token at once.
//! `HOLON_NET_TWIN=drop-late-done` / `drop-late-failed` drop the late Done /
//! Failed of a parked attempt. `HOLON_NET_TWIN=failed-settles-alone` lets a
//! Failed settle the occurrence while another attempt may have happened.

mod exec_support;

use std::collections::BTreeMap;
use std::time::Duration;

use exec_support::Driver;
use exec_support::Reads;
use exec_support::Spec;
use exec_support::Transition;
use exec_support::bind;
use exec_support::content;
use exec_support::string;
use holon_api::Value;
use holon_net::exec::AttemptId;
use holon_net::exec::Body;
use holon_net::exec::Cause;
use holon_net::exec::Class;
use holon_net::exec::Completion;
use holon_net::exec::Content;
use holon_net::exec::EffectRequest;
use holon_net::exec::EffectResult;
use holon_net::exec::ExecError;
use holon_net::exec::FireError;
use holon_net::exec::OUTCOME_UNKNOWN;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Output;
use holon_net::exec::Receiver;
use holon_net::exec::Resolution;
use holon_net::exec::Settled;
use holon_net::exec::Subject;
use holon_net::exec::Trigger;
use proptest::prelude::*;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;
use proptest_state_machine::prop_state_machine;

const APP: &str = "app";
const RUN_TIMER: u64 = 20;
const ABANDON: u64 = 3;
const PRODUCERS: [&str; 2] = ["FillFromTurso", "MakeLoroReady"];

fn gen_of(r: &mut dyn Reads, s: &str) -> Option<Content> {
    Some(content(&[("gen", r.field("Storage.Open", s, "gen")?)]))
}

fn storage(backend: &str, input: &Content) -> Content {
    let mut stored = input.clone();
    stored.insert("backend".to_string(), Value::String(backend.to_string()));
    stored
}

/// Example 3 with a run timer and an abandon timer on the Turso attempt;
/// `candidate` declares an apply transition for `TursoReady`.
fn net(candidate: bool) -> Spec {
    let net = Spec::default();
    let net = if candidate {
        net.place_with_candidate("TursoReady", "TursoReady.Candidate")
            .fifo("TursoReady.Candidate", Class::Process)
    } else {
        net.place("TursoReady", Class::Process)
    };
    net.place("Storage", Class::Process)
        .place("Storage.Open", Class::Process)
        .place("TursoFailed", Class::Process)
        .transition(
            Transition::new(
                "MakeTursoReady",
                Trigger::Auto,
                bind!(gen_of),
                Body::Effectful(Receiver::None),
            )
            .reads("Storage.Open")
            .inhibits("TursoReady")
            .inhibits("TursoFailed")
            .on_failure("TursoFailed")
            .run_timer(Duration::from_secs(RUN_TIMER))
            .on_timeout("TursoFailed")
            .abandon(Duration::from_secs(ABANDON)),
        )
        .transition(
            Transition::new(
                "FillFromTurso",
                Trigger::Auto,
                bind!(gen_of),
                Body::pure(|s, input| Ok(vec![Output::new("Storage", s, storage("turso", input))])),
            )
            .consumes("TursoReady")
            .consumes("Storage.Open"),
        )
        .transition(
            Transition::new(
                "MakeLoroReady",
                Trigger::Auto,
                bind!(gen_of),
                Body::pure(|s, input| Ok(vec![Output::new("Storage", s, storage("loro", input))])),
            )
            .consumes("TursoFailed")
            .consumes("Storage.Open"),
        )
}

#[derive(Clone, Debug)]
enum Op {
    Done,
    Failed,
    Unknown,
    RunTimerExpires,
    Abandon,
    /// The runner answers the newer pending effect first.
    Reorder,
    Verify,
    Retry,
    Drop,
}

struct Ref;

impl ReferenceStateMachine for Ref {
    type State = ();
    type Transition = Op;

    fn init_state() -> BoxedStrategy<()> {
        Just(()).boxed()
    }

    fn transitions(_: &()) -> BoxedStrategy<Op> {
        prop_oneof![
            Just(Op::Done),
            Just(Op::Failed),
            Just(Op::Unknown),
            Just(Op::RunTimerExpires),
            Just(Op::Abandon),
            Just(Op::Reorder),
            Just(Op::Verify),
            Just(Op::Retry),
            Just(Op::Drop),
        ]
        .boxed()
    }

    fn apply(state: (), _: &Op) {
        state
    }
}

struct Sut {
    d: Driver,
    /// A definite failure, a Drop or the run timer has released the next
    /// producer.
    released: bool,
    /// The answer the runner gave each attempt, by its result kind.
    answers: BTreeMap<AttemptId, &'static str>,
}

impl Sut {
    fn parked(&self) -> Option<OccurrenceId> {
        self.d
            .exec()
            .occurrences()
            .find(|(_, s)| matches!(s, Some(Settled::OutcomeUnknown(_))))
            .map(|(o, _)| o.clone())
    }

    fn backend(&self) -> Option<String> {
        self.d
            .exec()
            .tokens()
            .token("Storage", APP)
            .map(|t| string(&t.content, "backend"))
    }

    fn resolve(&mut self, resolution: Resolution) {
        let occ = self.parked().expect("checked by the caller");
        match self.d.resolve(&occ, resolution) {
            Ok(()) | Err(ExecError::RetryWhileRunning { .. }) => {}
            Err(e) => panic!("the occurrence is parked: {e}"),
        }
    }

    /// Every attempt of `occ` was answered Failed: none can still have
    /// taken effect.
    fn all_failed(&self, occ: &OccurrenceId) -> bool {
        self.d
            .requests
            .iter()
            .filter(|r| &r.occurrence == occ)
            .all(|r| self.answers.get(&r.attempt) == Some(&"failed"))
    }

    /// Answers the oldest pending effect and records the answer.
    fn answer(&mut self, kind: &'static str, result: impl FnOnce(&EffectRequest) -> EffectResult) {
        if let Some(req) = self.d.pending.front() {
            self.answers.insert(req.attempt, kind);
        }
        self.d.complete_oldest(result).expect("the net quiesces");
    }
}

struct Test<const CANDIDATE: bool>;

impl<const CANDIDATE: bool> StateMachineTest for Test<CANDIDATE> {
    type SystemUnderTest = Sut;
    type Reference = Ref;

    fn init_test(_: &()) -> Sut {
        let mut d = Driver::new(net(CANDIDATE));
        d.write("Storage.Open", APP, content(&[("gen", Value::Integer(0))]))
            .expect("the net quiesces");
        Sut {
            d,
            released: false,
            answers: BTreeMap::new(),
        }
    }

    fn apply(mut sut: Sut, _: &(), op: Op) -> Sut {
        match op {
            Op::Done => sut.answer("done", |req| {
                EffectResult::Done(vec![Output::new(
                    "TursoReady",
                    &req.subject,
                    req.input.clone(),
                )])
            }),
            Op::Failed => {
                let occ = sut.d.pending.front().map(|r| r.occurrence.clone());
                sut.answer("failed", |_| {
                    EffectResult::Failed(Cause::definite("no database"))
                });
                if let Some(occ) = occ
                    && sut.all_failed(&occ)
                {
                    sut.released = true;
                }
            }
            Op::Unknown => sut.answer("unknown", |_| {
                EffectResult::Unknown(Cause::definite("connection reset"))
            }),
            Op::RunTimerExpires => {
                sut.released = true;
                sut.d.tick(RUN_TIMER + 1).expect("the net quiesces");
            }
            Op::Abandon => {
                sut.d.tick(ABANDON + 1).expect("the net quiesces");
                // A run starts at t >= 0, so no run timer expires before RUN_TIMER.
                if sut.d.now >= Duration::from_secs(RUN_TIMER) {
                    sut.released = true;
                }
            }
            Op::Reorder => {
                if !sut.d.pending.is_empty() {
                    sut.d.pending.rotate_left(1);
                }
            }
            Op::Verify => {
                if let Some(occ) = sut.parked() {
                    let input = sut
                        .d
                        .requests
                        .iter()
                        .find(|r| r.occurrence == occ)
                        .expect("a parked occurrence ran")
                        .input
                        .clone();
                    sut.resolve(Resolution::Verify(vec![Output::new(
                        "TursoReady",
                        &Subject::new(APP),
                        input,
                    )]));
                }
            }
            Op::Retry => {
                if sut.parked().is_some() {
                    sut.resolve(Resolution::Retry);
                }
            }
            Op::Drop => {
                if sut.parked().is_some() {
                    sut.released = true;
                    sut.resolve(Resolution::Drop);
                }
            }
        }
        sut
    }

    fn check_invariants(sut: &Sut, _: &()) {
        if sut.backend().as_deref() == Some("loro") {
            assert!(
                sut.released,
                "unknown_fires_no_next_producer: MakeLoroReady fired while the Turso outcome \
                 was only unknown"
            );
        }
        let products = sut
            .d
            .exec()
            .occurrences()
            .filter(|(o, s)| {
                matches!(o, OccurrenceId::Minted { transition, .. } if PRODUCERS.contains(&&**transition))
                    && matches!(s, Some(Settled::Done))
            })
            .count();
        assert!(
            products <= 1,
            "one_storage_product: {products} producers stored a Storage token"
        );
        if sut.backend().is_some() {
            assert!(
                sut.d.exec().tokens().token("TursoReady", APP).is_none(),
                "one_storage_product: a TursoReady token waits beside the Storage product"
            );
        }
    }
}

prop_state_machine! {
    #[test]
    fn a_late_verify_without_candidate_is_a_disclosed_condition(sequential 1..30 => Test<false>);
}

prop_state_machine! {
    #[test]
    fn a_late_verify_with_candidate_becomes_a_candidate(sequential 1..30 => Test<true>);
}

/// A late Done of an abandoned, parked attempt is evidence that the effect
/// happened: it resolves the occurrence like a Verify (NET-fire.a).
#[test]
fn a_late_done_resolves_the_parked_occurrence() {
    let mut d = Driver::new(net(false));
    d.write("Storage.Open", APP, content(&[("gen", Value::Integer(0))]))
        .expect("the net quiesces");
    let occ = d
        .pending
        .front()
        .expect("MakeTursoReady fired")
        .occurrence
        .clone();
    d.tick(ABANDON + 1).expect("the net quiesces");
    assert!(
        matches!(d.exec().settled(&occ), Ok(Some(Settled::OutcomeUnknown(_)))),
        "the abandoned attempt parks"
    );
    d.tick(1).expect("the net quiesces");
    d.complete_oldest(|req| {
        EffectResult::Done(vec![Output::new(
            "TursoReady",
            &req.subject,
            req.input.clone(),
        )])
    })
    .expect("the net quiesces");

    assert_eq!(
        d.exec().settled(&occ),
        Ok(Some(Settled::Done)),
        "late_done_resolves: the late Done of {occ}"
    );
    assert_eq!(
        d.exec().tokens().subjects(holon_net::exec::OUTCOME_UNKNOWN),
        Vec::<Subject>::new(),
        "late_done_clears_unknown: OutcomeUnknown still holds a token"
    );
    let backends: Vec<String> = d
        .exec()
        .tokens()
        .token("Storage", APP)
        .map(|t| string(&t.content, "backend"))
        .into_iter()
        .collect();
    assert_eq!(backends, vec!["turso".to_string()], "late_done_one_product");
}

/// A manual `Send` with an abandon timer that consumes `Msg`, fired; a
/// `Keyed` one re-issues once.
fn fired_send(receiver: Receiver) -> (Driver, EffectRequest) {
    let send = Transition::new(
        "Send",
        Trigger::Manual,
        bind!(|r, s| Some(content(&[("n", r.field("Msg", s, "n")?)]))),
        Body::Effectful(receiver),
    )
    .consumes("Msg")
    .abandon(Duration::from_secs(ABANDON));
    let send = if receiver == Receiver::Keyed {
        send.retry(Duration::ZERO, 2)
    } else {
        send
    };
    let net = Spec::default()
        .place("Msg", Class::Process)
        .place("Sent", Class::Process)
        .transition(send);
    let mut d = Driver::new(net);
    d.write("Msg", APP, content(&[("n", Value::Integer(1))]))
        .expect("the net quiesces");
    d.fire("Send", APP, OccurrenceId::caller("send#1"))
        .expect("Msg holds a message");
    let req = d.pending.front().expect("Send runs").clone();
    (d, req)
}

/// A `Send` with receiver `None`, fired, abandoned and parked.
fn parked_send() -> (Driver, EffectRequest) {
    let (mut d, req) = fired_send(Receiver::None);
    d.tick(ABANDON + 1).expect("the net quiesces");
    assert!(
        matches!(
            d.exec().settled(&req.occurrence),
            Ok(Some(Settled::OutcomeUnknown(_)))
        ),
        "the abandoned attempt parks"
    );
    d.tick(1).expect("the net quiesces");
    (d, req)
}

/// One subject, one effect in flight: Retry of a parked `Send` waits while a
/// newer `Send` runs, and is offered again once that one settles.
#[test]
fn retry_waits_for_a_newer_firing_in_flight() {
    let (mut d, first) = parked_send();
    d.write("Msg", APP, content(&[("n", Value::Integer(2))]))
        .expect("the net quiesces");
    d.fire("Send", APP, OccurrenceId::caller("send#2"))
        .expect("Msg holds a newer message");
    let newer = d.pending.back().expect("the newer Send runs").clone();
    assert_eq!(
        d.resolve(&first.occurrence, Resolution::Retry),
        Err(ExecError::RetryWhileRunning {
            transition: "Send".to_string(),
            subject: Subject::new(APP),
        })
    );
    answer_attempt(&mut d, &newer, sent(&newer));
    d.resolve(&first.occurrence, Resolution::Retry)
        .expect("the newer firing settled");
}

/// A late Failed is evidence that the effect did not happen: the input
/// returns, the mark is set and OutcomeUnknown is cleared.
#[test]
fn a_late_failed_returns_the_input() {
    let (mut d, req) = parked_send();
    d.complete_oldest(|_| EffectResult::Failed(Cause::definite("rejected")))
        .expect("the net quiesces");

    let settled = d.exec().settled(&req.occurrence);
    assert!(
        matches!(settled, Ok(Some(Settled::Failed(_)))),
        "late_failed_resolves: the late Failed left {settled:?}"
    );
    assert!(
        d.exec().tokens().token("Msg", APP).is_some(),
        "late_failed_returns_input: Msg is empty"
    );
    assert_eq!(
        d.exec().mark("Send", APP).map(|m| m.cause.text.as_str()),
        Some("rejected"),
        "late_failed_marks"
    );
    assert_eq!(
        d.exec().tokens().subjects(OUTCOME_UNKNOWN),
        Vec::<Subject>::new(),
        "late_failed_clears_unknown"
    );
}

/// A late Unknown adds no evidence: the occurrence stays parked, and a later
/// result of the same attempt still resolves it.
#[test]
fn a_late_unknown_keeps_the_occurrence_parked() {
    let (mut d, req) = parked_send();
    let late = d.complete(req.attempt, EffectResult::Unknown(Cause::definite("reset")));
    assert_eq!(late, Ok(Completion::StillUnknown));
    assert!(
        matches!(
            d.exec().settled(&req.occurrence),
            Ok(Some(Settled::OutcomeUnknown(_)))
        ),
        "late_unknown_stays_parked: {:?}",
        d.exec().settled(&req.occurrence)
    );
    assert_eq!(
        d.exec().tokens().subjects(OUTCOME_UNKNOWN),
        vec![Subject::new(&req.occurrence.to_string())],
        "late_unknown_stays_disclosed"
    );

    let done = d.complete(
        req.attempt,
        EffectResult::Done(vec![Output::new("Sent", &req.subject, req.input.clone())]),
    );
    assert_eq!(done, Ok(Completion::Recorded));
    assert_eq!(d.exec().settled(&req.occurrence), Ok(Some(Settled::Done)));
    assert!(
        d.exec().tokens().token("Sent", APP).is_some(),
        "the later Done recorded"
    );
    assert_eq!(
        d.complete(req.attempt, EffectResult::Done(Vec::new())),
        Err(ExecError::UnknownAttempt(req.attempt)),
        "an attempt has one final result"
    );
}

fn answer_attempt(d: &mut Driver, req: &EffectRequest, result: EffectResult) -> Completion {
    d.complete(req.attempt, result).expect("the attempt ran")
}

/// A parked `Send` the user retried: the abandoned first attempt and the
/// running second one.
fn retried_send() -> (Driver, EffectRequest, EffectRequest) {
    let (mut d, first) = parked_send();
    d.resolve(&first.occurrence, Resolution::Retry)
        .expect("the occurrence is parked");
    let second = d.pending.back().expect("the retry runs").clone();
    assert_ne!(first.attempt, second.attempt);
    (d, first, second)
}

fn sent(req: &EffectRequest) -> EffectResult {
    EffectResult::Done(vec![Output::new("Sent", &req.subject, req.input.clone())])
}

fn rejected() -> EffectResult {
    EffectResult::Failed(Cause::definite("rejected"))
}

fn assert_still_parked(d: &mut Driver, occ: &OccurrenceId, rule: &str) {
    assert!(
        matches!(d.exec().settled(occ), Ok(Some(Settled::OutcomeUnknown(_)))),
        "{rule}: {occ} settled {:?}",
        d.exec().settled(occ)
    );
    assert!(
        d.exec().tokens().token("Msg", APP).is_none(),
        "{rule}: the input returned"
    );
    assert!(
        matches!(
            d.fire("Send", APP, OccurrenceId::caller("send#2")),
            Err(FireError::NotEnabled { .. })
        ),
        "{rule}: Send can fire again"
    );
}

/// An older attempt's Failed says nothing about a newer attempt whose outcome
/// is unknown: the occurrence stays parked and nothing can send again.
#[test]
fn an_older_failed_does_not_settle_a_newer_unknown() {
    let (mut d, first, second) = retried_send();
    answer_attempt(
        &mut d,
        &second,
        EffectResult::Unknown(Cause::definite("reset")),
    );
    assert_eq!(
        answer_attempt(&mut d, &first, rejected()),
        Completion::StillUnknown
    );
    assert_still_parked(
        &mut d,
        &first.occurrence,
        "failed_only_when_every_attempt_failed",
    );
}

/// The same for a `Keyed` re-issue after the abandon timer.
#[test]
fn an_older_failed_does_not_settle_a_keyed_reissue() {
    let (mut d, first) = fired_send(Receiver::Keyed);
    d.tick(ABANDON + 1).expect("the net quiesces");
    let second = d
        .pending
        .back()
        .expect("the keyed receiver re-issues")
        .clone();
    assert_ne!(first.attempt, second.attempt);
    answer_attempt(
        &mut d,
        &second,
        EffectResult::Unknown(Cause::definite("reset")),
    );
    answer_attempt(&mut d, &first, rejected());
    assert_still_parked(
        &mut d,
        &first.occurrence,
        "failed_only_when_every_attempt_failed",
    );
}

/// Any Done makes the occurrence Done, whatever an older attempt answered.
#[test]
fn an_older_failed_and_a_newer_done_is_done() {
    let (mut d, first, second) = retried_send();
    assert_eq!(
        answer_attempt(&mut d, &first, rejected()),
        Completion::StillUnknown
    );
    assert_eq!(
        d.exec().settled(&first.occurrence),
        Ok(None),
        "the retry still runs"
    );
    assert_eq!(
        answer_attempt(&mut d, &second, sent(&second)),
        Completion::Recorded
    );
    assert_eq!(d.exec().settled(&first.occurrence), Ok(Some(Settled::Done)));
    assert!(
        d.exec().tokens().token("Sent", APP).is_some(),
        "done_when_any_attempt_done"
    );
}

/// Only when every attempt failed does the input return and the mark set.
#[test]
fn every_attempt_failed_returns_the_input() {
    let (mut d, first, second) = retried_send();
    answer_attempt(&mut d, &second, rejected());
    assert_still_parked(
        &mut d,
        &first.occurrence,
        "failed_only_when_every_attempt_failed",
    );
    answer_attempt(&mut d, &first, rejected());
    assert!(
        matches!(
            d.exec().settled(&first.occurrence),
            Ok(Some(Settled::Failed(_)))
        ),
        "all_failed_settles_failed: {:?}",
        d.exec().settled(&first.occurrence)
    );
    assert!(
        d.exec().tokens().token("Msg", APP).is_some(),
        "all_failed_returns_input"
    );
    assert_eq!(
        d.exec().mark("Send", APP).map(|m| m.cause.text.as_str()),
        Some("rejected"),
        "all_failed_marks"
    );
    assert_eq!(
        d.exec().tokens().subjects(OUTCOME_UNKNOWN),
        Vec::<Subject>::new(),
        "all_failed_clears_unknown"
    );
}
