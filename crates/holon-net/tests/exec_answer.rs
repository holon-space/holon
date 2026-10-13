//! Answers (UFT-adopt.a, ADR 0032 §9.2 NET-manual.a): the user value joins
//! the work input after the bind, `settled(answer)` follows the work, a new
//! answer is a new failure mark key, and undo never fires the work again.
//!
//! Twins: `HOLON_NET_TWIN=value-before-bind`, `occurrence-not-in-trace`,
//! `undo-restores-answered`, `settled-from-answer`.

mod exec_support;

use std::collections::BTreeMap;
use std::sync::Arc;

use exec_support::Driver;
use exec_support::Transition;
use exec_support::answer_call;
use exec_support::answer_spec;
use exec_support::bind;
use exec_support::content;
use exec_support::int;
use exec_support::text;
use holon_api::Value;
use holon_api::user_transition::SubjectName;
use holon_api::user_transition::TransitionName;
use holon_api::user_transition::UftError;
use holon_net::exec::Body;
use holon_net::exec::Cause;
use holon_net::exec::Class;
use holon_net::exec::Condition;
use holon_net::exec::Content;
use holon_net::exec::EffectResult;
use holon_net::exec::MAX_ROUNDS;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Output;
use holon_net::exec::Place;
use holon_net::exec::Subject;
use holon_net::exec::Trigger;
use holon_net::exec::answer_transition;
use holon_net::exec::answered_place;
use proptest::prelude::*;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;
use proptest_state_machine::prop_state_machine;

const SUBJECTS: [&str; 2] = ["x", "y"];
const IDS: [&str; 4] = ["a0", "a1", "a2", "a3"];

#[derive(Clone, Debug)]
enum Op {
    WriteItem {
        s: &'static str,
        n: i64,
    },
    Answer {
        s: &'static str,
        id: &'static str,
        stale: bool,
        value: Option<&'static str>,
    },
    Undo(&'static str),
    CompleteDone,
    CompleteFailed,
    CompleteUnknown,
}

struct Ref;

impl ReferenceStateMachine for Ref {
    type State = ();
    type Transition = Op;

    fn init_state() -> BoxedStrategy<()> {
        Just(()).boxed()
    }

    fn transitions(_: &()) -> BoxedStrategy<Op> {
        let s = prop::sample::select(SUBJECTS.to_vec());
        let id = prop::sample::select(IDS.to_vec());
        prop_oneof![
            1 => (s.clone(), -1..3i64).prop_map(|(s, n)| Op::WriteItem { s, n }),
            3 => (
                s,
                id.clone(),
                prop::bool::weighted(0.2),
                prop::option::of(prop::sample::select(vec!["Mine", "Theirs"])),
            )
                .prop_map(|(s, id, stale, value)| Op::Answer {
                    s,
                    id,
                    stale,
                    value
                }),
            1 => id.prop_map(Op::Undo),
            2 => Just(Op::CompleteDone),
            2 => Just(Op::CompleteFailed),
            1 => Just(Op::CompleteUnknown),
        ]
        .boxed()
    }

    fn apply(state: (), _: &Op) {
        state
    }
}

struct Sut {
    d: Driver,
    /// The value of each accepted answer.
    values: BTreeMap<String, Value>,
}

/// An ask for (T, s) is offered only while (T, s) is idle: no `Answered.T`,
/// no work in flight, no `WorkFailed.T` (`answer_spec` has no retry). An
/// offered ask has its Answer enabled: an external call returns only after
/// the auto transitions it caused, so no answer waits for a `step` (D151.a,
/// #167 idle rule, #192).
fn offered_asks_have_their_answer_enabled(d: &Driver, op: &str) {
    for row in d.exec().asks() {
        let (work, s) = (&row.transition.0, &row.subject.0);
        let tokens = d.exec().tokens();
        for busy in [
            answered_place(work),
            Place::running(work).to_string(),
            format!("WorkFailed.{work}"),
        ] {
            assert!(
                tokens.token(&busy, s).is_none(),
                "ask of '{work}' offered for {s} while {busy} holds, after {op}"
            );
        }
        let answer = answer_transition(work);
        let why = d.exec().why_not(&answer, s);
        assert!(
            why.is_none(),
            "ask offered but its Answer is not enabled: '{answer}' for {s} after {op}: {why:?}"
        );
    }
}

/// Each answer fires its work at most once, with its own value.
fn work_fired_once_with_its_value(sut: &Sut) {
    let mut seen = BTreeMap::new();
    for req in sut.d.requests_of("Accept") {
        let OccurrenceId::Work { answer, .. } = &req.occurrence else {
            panic!("the work runs under {}", req.occurrence);
        };
        assert!(
            seen.insert(answer.to_string(), ()).is_none(),
            "the same answer fired twice: {}",
            req.occurrence
        );
        assert_eq!(
            req.input.get(holon_api::user_transition::ANSWER_INPUT_KEY),
            sut.values.get(answer.as_ref()),
            "the work of {answer} has the wrong value"
        );
    }
}

struct Test;

impl StateMachineTest for Test {
    type SystemUnderTest = Sut;
    type Reference = Ref;

    fn init_test(_: &()) -> Sut {
        Sut {
            d: Driver::new(answer_spec()),
            values: BTreeMap::new(),
        }
    }

    fn apply(mut sut: Sut, _: &(), op: Op) -> Sut {
        let d = &mut sut.d;
        let op_text = format!("{op:?}");
        match op {
            Op::WriteItem { s, n } => {
                d.write("Item", s, content(&[("n", Value::Integer(n))]))
                    .expect("the net quiesces");
            }
            Op::Answer {
                s,
                id,
                stale,
                value,
            } => {
                let c = answer_call(d, s, id, stale, value);
                if d.answer(c).is_ok() {
                    sut.values
                        .insert(id.to_string(), value.map_or(Value::Null, text));
                }
            }
            Op::Undo(id) => {
                let _disclosed_or_refused = d.undo(&OccurrenceId::caller(id));
            }
            Op::CompleteDone => d
                .complete_oldest(|req| {
                    let n = Value::Integer(int(&req.input, "n"));
                    EffectResult::Done(vec![
                        Output::new("Accepted", &req.subject, content(&[("n", n.clone())])),
                        Output::new("Out", &req.subject, content(&[("n", n)])),
                    ])
                })
                .expect("the net quiesces"),
            Op::CompleteFailed => d
                .complete_oldest(|_| EffectResult::Failed(Cause::definite("rejected")))
                .expect("the net quiesces"),
            Op::CompleteUnknown => d
                .complete_oldest(|_| EffectResult::Unknown(Cause::definite("reset")))
                .expect("the net quiesces"),
        }
        offered_asks_have_their_answer_enabled(&sut.d, &op_text);
        work_fired_once_with_its_value(&sut);
        sut
    }
}

prop_state_machine! {
    #![proptest_config(ProptestConfig {
        cases: 512,
        .. ProptestConfig::default()
    })]

    #[test]
    fn answers_follow_the_model(sequential 1..30 => Test);
}

/// An external call returns only after the auto transitions it caused are
/// quiescent (#192).
#[test]
fn an_answer_returns_after_quiescence() {
    let mut d = Driver::new(answer_spec());
    d.write("Item", "x", content(&[("n", Value::Integer(1))]))
        .expect("the net quiesces");
    let c = answer_call(&d, "x", "a0", false, None);
    d.answer(c).expect("the ask applies");
    assert!(
        d.exec().tokens().token("Answered.Accept", "x").is_none(),
        "the auto work Accept has not taken Answered.Accept/x when answer returns"
    );
}

/// An offered ask has its Answer enabled after every call, with no `step`
/// between the calls: a failed work is released at once.
#[test]
fn an_offered_ask_has_its_answer_enabled() {
    let mut d = Driver::new(answer_spec());
    d.write("Item", "x", content(&[("n", Value::Integer(1))]))
        .expect("the net quiesces");
    let c = answer_call(&d, "x", "a0", false, None);
    d.answer(c).expect("the ask applies");
    offered_asks_have_their_answer_enabled(&d, "the answer a0");
    let attempt = d.pending.front().expect("the work is running").attempt;
    d.complete(attempt, EffectResult::Failed(Cause::definite("rejected")))
        .expect("the attempt is in flight");
    let offered = d.exec().asks();
    assert_eq!(
        offered.len(),
        1,
        "the ask of x is offered again: {offered:?}"
    );
    assert_eq!(d.exec().why_not(&answer_transition("Accept"), "x"), None);
}

/// A fixed path through the red of each twin: answer, the work fails, the
/// user answers again, the work runs and is done, the user undoes it.
#[test]
fn answer_fail_reanswer_done_undo() {
    let mut d = Driver::new(answer_spec());
    d.write("Item", "x", content(&[("n", Value::Integer(1))]))
        .expect("the net quiesces");
    let c = answer_call(&d, "x", "a0", false, Some("Theirs"));
    d.answer(c).expect("the ask applies");
    d.complete_oldest(|_| EffectResult::Failed(Cause::definite("rejected")))
        .expect("the net quiesces");
    let c = answer_call(&d, "x", "a1", false, None);
    d.answer(c).expect("a failed answer was released");
    d.complete_oldest(|req| {
        EffectResult::Done(vec![Output::new(
            "Accepted",
            &req.subject,
            content(&[("n", Value::Integer(1))]),
        )])
    })
    .expect("the net quiesces");
    d.undo(&OccurrenceId::caller("a1"))
        .expect("a done answer undoes");
    assert_eq!(d.requests_of("Accept").count(), 2);
}

/// The PBT trace that offered an ask while the work of its subject ran: the
/// ask of `y` comes back only when the work of `a1` settles.
#[test]
fn no_ask_is_offered_while_its_work_runs() {
    let mut d = Driver::new(answer_spec());
    let item = || content(&[("n", Value::Integer(0))]);
    d.write("Item", "y", item()).expect("the net quiesces");
    let c = answer_call(&d, "y", "a1", false, None);
    d.answer(c).expect("the ask applies");
    d.write("Item", "y", item()).expect("the net quiesces");
    assert_eq!(d.exec().asks(), Vec::new(), "an ask of y while a1 works");
    let c = answer_call(&d, "y", "a0", false, None);
    assert_eq!(d.answer(c), Err(UftError::NoLongerApplies));
    d.complete_oldest(|_| EffectResult::Failed(Cause::definite("rejected")))
        .expect("the net quiesces");
    offered_asks_have_their_answer_enabled(&d, "the failure of a1");
    assert_eq!(d.exec().asks().len(), 1, "the ask of y is back");
}

/// `answer_spec` whose work also reads the absent `Gate`, and whose auto
/// `Loop` fires on every round while an answer waits: no answer quiesces.
fn spinning() -> Driver {
    let mut spec = answer_spec()
        .place("Gate", Class::Process)
        .place("Spin", Class::Process)
        .transition(
            Transition::new(
                "Loop",
                Trigger::Auto,
                bind!(|r, s| r.present("Answered.Accept", s).then(Content::new)),
                Body::pure(|s, input| Ok(vec![Output::new("Spin", s, input.clone())])),
            )
            .reads("Answered.Accept"),
        );
    spec.transitions
        .iter_mut()
        .find(|t| t.name == "Accept")
        .expect("answer_spec has the work Accept")
        .reads
        .push("Gate".to_string());
    let mut d = Driver::new(spec);
    d.write("Item", "x", content(&[("n", Value::Integer(1))]))
        .expect("the net quiesces");
    d
}

/// NET-rev1 termination: an answer that never quiesces is a typed error to
/// the caller, never a panic.
#[test]
fn an_answer_that_never_quiesces_is_a_typed_error() {
    let mut d = spinning();
    let c = answer_call(&d, "x", "a0", false, None);
    assert_eq!(
        d.answer(c),
        Err(UftError::NoQuiescence {
            transition: TransitionName("Accept".to_string()),
            subject: SubjectName("x".to_string()),
            steps: MAX_ROUNDS,
        })
    );
    assert_eq!(
        d.exec().conditions(),
        [Condition::NoQuiescence {
            work: Arc::from("Accept"),
            subject: Subject::new("x"),
            steps: MAX_ROUNDS,
        }],
        "the subject discloses the condition"
    );
}

/// `answer_spec` whose work also reads the absent `Gate`, so an answer stays
/// pending in `Answered.Accept`.
fn gated() -> Driver {
    let mut spec = answer_spec().place("Gate", Class::Process);
    spec.transitions
        .iter_mut()
        .find(|t| t.name == "Accept")
        .expect("answer_spec has the work Accept")
        .reads
        .push("Gate".to_string());
    let mut d = Driver::new(spec);
    d.write("Item", "x", content(&[("n", Value::Integer(1))]))
        .expect("the net quiesces");
    d
}

/// A pending answer makes its subject busy: no ask until the work takes it.
#[test]
fn no_ask_is_offered_while_an_answer_is_pending() {
    let mut d = gated();
    let c = answer_call(&d, "x", "a0", false, None);
    d.answer(c).expect("the ask applies");
    assert!(d.exec().tokens().token("Answered.Accept", "x").is_some());
    offered_asks_have_their_answer_enabled(&d, "the answer a0");
    d.write("Gate", "x", Content::new())
        .expect("the net quiesces");
    assert!(d.exec().tokens().token("Answered.Accept", "x").is_none());
}

/// A failed work makes its subject busy until `ReleaseFailed.Accept` clears
/// it; here a `Hold` keeps it from firing.
#[test]
fn no_ask_is_offered_while_its_work_failed() {
    let mut spec = answer_spec().place("Hold", Class::Process);
    spec.transitions
        .iter_mut()
        .find(|t| t.name == "ReleaseFailed.Accept")
        .expect("answer_spec releases failed answers")
        .inhibits
        .push("Hold".to_string());
    let mut d = Driver::new(spec);
    d.write("Hold", "x", Content::new())
        .expect("the net quiesces");
    d.write("Item", "x", content(&[("n", Value::Integer(1))]))
        .expect("the net quiesces");
    let c = answer_call(&d, "x", "a0", false, None);
    d.answer(c).expect("the ask applies");
    d.complete_oldest(|_| EffectResult::Failed(Cause::definite("rejected")))
        .expect("the net quiesces");
    assert!(d.exec().tokens().token("WorkFailed.Accept", "x").is_some());
    offered_asks_have_their_answer_enabled(&d, "the failure of a0");
}
