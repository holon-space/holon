//! Answers (UFT-adopt.a, ADR 0032 §9.2 NET-manual.a): the user value joins
//! the work input after the bind, `settled(answer)` follows the work, a new
//! answer is a new failure mark key, and undo never fires the work again.
//!
//! Twins: `HOLON_NET_TWIN=value-before-bind`, `occurrence-not-in-trace`,
//! `undo-restores-answered`, `settled-from-answer`.

mod exec_support;

use std::collections::BTreeMap;

use exec_support::Driver;
use exec_support::answer_call;
use exec_support::answer_spec;
use exec_support::content;
use exec_support::int;
use exec_support::text;
use holon_api::Value;
use holon_net::exec::Cause;
use holon_net::exec::EffectResult;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Output;
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
            req.input.get(holon_net::exec::VALUE_FIELD),
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
        match op {
            Op::WriteItem { s, n } => {
                d.write("Item", s, content(&[("n", Value::Integer(n))]));
                d.step().expect("the net quiesces");
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
                d.step().expect("the net quiesces");
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

/// A fixed path through the red of each twin: answer, the work fails, the
/// user answers again, the work runs and is done, the user undoes it.
#[test]
fn answer_fail_reanswer_done_undo() {
    let mut d = Driver::new(answer_spec());
    d.write("Item", "x", content(&[("n", Value::Integer(1))]));
    d.step().expect("the net quiesces");
    let c = answer_call(&d, "x", "a0", false, Some("Theirs"));
    d.answer(c).expect("the ask applies");
    d.step().expect("the net quiesces");
    d.complete_oldest(|_| EffectResult::Failed(Cause::definite("rejected")))
        .expect("the net quiesces");
    let c = answer_call(&d, "x", "a1", false, None);
    d.answer(c).expect("a failed answer was released");
    d.step().expect("the net quiesces");
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
