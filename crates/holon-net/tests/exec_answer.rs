//! Answers (UFT-adopt.a, ADR 0032 §9.2 NET-manual.a): the user value joins
//! the work input after the bind, `settled(answer)` follows the work, a new
//! answer is a new failure mark key, and undo never fires the work again.
//!
//! Twins: `HOLON_NET_TWIN=value-before-bind`, `occurrence-not-in-trace`,
//! `undo-restores-answered`, `settled-from-answer`; `no-quiescence` and
//! `keep-work-failed` break quiescence, which
//! `idle_rule_and_quiescence_hold` names without the model comparison.
//!
//! The net shape is drawn, not fixed (#195): the downstream consumer of
//! `Accepted` is part of the generated case, so an undo that finds a firing
//! holding the row the work wrote is reached by the generator and not only by
//! the deterministic `undo_while_a_downstream_firing_holds_a_row`.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use exec_support::Driver;
use exec_support::Spec;
use exec_support::Transition;
use exec_support::answer_call;
use exec_support::answer_spec;
use exec_support::bind;
use exec_support::content;
use exec_support::int;
use exec_support::text;
use holon_api::Value;
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
use holon_net::exec::Receiver;
use holon_net::exec::Resolution;
use holon_net::exec::ReversalOutcome;
use holon_net::exec::Subject;
use holon_net::exec::Trigger;
use holon_net::exec::answer_transition;
use holon_net::exec::answered_place;
use holon_transition_contract::SubjectName;
use holon_transition_contract::TransitionName;
use holon_transition_contract::UftError;
use proptest::prelude::*;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;
use proptest_state_machine::prop_state_machine;

use crate::exec_support;

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
    /// Undoes the answer accepted last: it reaches the state where a
    /// downstream firing still holds the row that work wrote.
    UndoNewest,
    CompleteDone,
    CompleteFailed,
    CompleteUnknown,
}

/// How a drawn downstream transition runs: `Effectful` can hold a row it took
/// across calls, `Pure` fires and finishes inside one quiescence.
#[derive(Clone, Copy, Debug)]
enum Kind {
    Effectful,
    Pure,
}

/// The drawn net shape: `answer_spec` plus the downstream consumer of
/// `Accepted`, the row the work writes. `Item`, the row the work consumes,
/// needs no knob: a consumer of `Item` can only take a row written after the
/// work, which bumps the slot version and makes the reversal `Changed`. The
/// holder of an `Item` row is an earlier answer's own parked work, which the
/// generator reaches without a knob.
#[derive(Clone, Copy, Debug)]
struct Shape {
    consume_accepted: Option<Kind>,
}

fn spec_of(shape: &Shape) -> Spec {
    let Some(kind) = shape.consume_accepted else {
        return answer_spec();
    };
    let body = match kind {
        Kind::Effectful => Body::Effectful(Receiver::None),
        Kind::Pure => Body::pure(|_, _| Ok(Vec::new())),
    };
    answer_spec().transition(
        Transition::new(
            "Consume",
            Trigger::Auto,
            bind!(|r, s| r.present("Accepted", s).then(Content::new)),
            body,
        )
        .consumes("Accepted"),
    )
}

struct Ref;

impl ReferenceStateMachine for Ref {
    type State = Shape;
    type Transition = Op;

    fn init_state() -> BoxedStrategy<Shape> {
        prop_oneof![
            1 => Just(None),
            2 => Just(Some(Kind::Effectful)),
            1 => Just(Some(Kind::Pure)),
        ]
        .prop_map(|consume_accepted| Shape { consume_accepted })
        .boxed()
    }

    fn transitions(shape: &Shape) -> BoxedStrategy<Op> {
        let s = prop::sample::select(SUBJECTS.to_vec());
        let id = prop::sample::select(IDS.to_vec());
        // A firing can hold a written row only in the effectful shape, so undo
        // of the newest answer is drawn there: that is where the held branch
        // lives, and it needs the undo right after the work was done.
        let undo_newest = if matches!(shape.consume_accepted, Some(Kind::Effectful)) {
            4
        } else {
            1
        };
        prop_oneof![
            2 => (s.clone(), -1..3i64).prop_map(|(s, n)| Op::WriteItem { s, n }),
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
            undo_newest => Just(Op::UndoNewest),
            4 => Just(Op::CompleteDone),
            2 => Just(Op::CompleteFailed),
            1 => Just(Op::CompleteUnknown),
        ]
        .boxed()
    }

    fn apply(state: Shape, _: &Op) -> Shape {
        state
    }
}

struct Sut {
    d: Driver,
    /// The value of each accepted answer.
    values: BTreeMap<String, Value>,
    /// The answer accepted last, the target of [`Op::UndoNewest`].
    newest: Option<&'static str>,
    /// Answers whose work overwrote an `Accepted` row that was already there.
    overwrote: BTreeSet<String>,
    held: Reach,
    /// The auto transitions of the drawn net.
    autos: Vec<String>,
}

/// How often `undo` reported a row a later firing holds, over the whole run.
/// Printed once, by the teardown of the last case: the weighting of the shape
/// and of `Undo` is shown, not assumed (#195).
#[derive(Default)]
struct Reach {
    rows: usize,
    /// Of those rows, the ones whose delta has `before = Some`: a row that
    /// existed before the work wrote it.
    before_some_rows: usize,
}

static CASES: AtomicUsize = AtomicUsize::new(0);
static HELD_ROWS: AtomicUsize = AtomicUsize::new(0);
static HELD_BEFORE_SOME_ROWS: AtomicUsize = AtomicUsize::new(0);
static HELD_CASES: AtomicUsize = AtomicUsize::new(0);
const CASES_RUN: u32 = 512;

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

/// Quiescence: an external call returns only after every auto transition it
/// enabled has fired, so none is enabled between calls (NET-rev1).
fn no_auto_transition_is_enabled(sut: &Sut, op: &str) {
    for t in &sut.autos {
        for s in SUBJECTS {
            assert!(
                sut.d.exec().why_not(t, s).is_some(),
                "not quiescent: the auto '{t}' is enabled for {s} after {op}"
            );
        }
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
            req.input.get(holon_transition_contract::ANSWER_INPUT_KEY),
            sut.values.get(answer.as_ref()),
            "the work of {answer} has the wrong value"
        );
    }
}

/// Undoes `id` and counts the rows a later firing holds.
fn undo_and_count(sut: &mut Sut, id: &str) {
    let Ok(undone) = sut.d.undo(&OccurrenceId::caller(id)) else {
        return;
    };
    if sut.newest == Some(id) {
        sut.newest = None;
    }
    for r in undone
        .reversed
        .iter()
        .filter(|r| matches!(r.outcome, ReversalOutcome::Held(_)))
    {
        sut.held.rows += 1;
        // The work consumes `Item`, so a held `Item` row is one that was there
        // before the work took it; a held `Accepted` row was only if the work
        // overwrote it.
        if &*r.place == "Item" || sut.overwrote.contains(id) {
            sut.held.before_some_rows += 1;
        }
    }
}

fn init_sut(spec: &Spec, d: Driver) -> Sut {
    Sut {
        d,
        autos: spec
            .transitions
            .iter()
            .filter(|t| t.trigger == Trigger::Auto)
            .map(|t| t.name.clone())
            .collect(),
        values: BTreeMap::new(),
        newest: None,
        overwrote: BTreeSet::new(),
        held: Reach::default(),
    }
}

/// The full oracle: the driver's model comparison, the idle rule,
/// quiescence and the work of each answer.
struct Test;

impl StateMachineTest for Test {
    type SystemUnderTest = Sut;
    type Reference = Ref;

    fn init_test(shape: &Shape) -> Sut {
        let spec = spec_of(shape);
        init_sut(&spec, Driver::new(spec.clone()))
    }

    fn teardown(sut: Sut, _: Shape) {
        HELD_ROWS.fetch_add(sut.held.rows, Ordering::Relaxed);
        HELD_BEFORE_SOME_ROWS.fetch_add(sut.held.before_some_rows, Ordering::Relaxed);
        if sut.held.rows > 0 {
            HELD_CASES.fetch_add(1, Ordering::Relaxed);
        }
        if CASES.fetch_add(1, Ordering::Relaxed) + 1 == CASES_RUN as usize {
            eprintln!(
                "held branch reached in {} of {CASES_RUN} cases: {} held rows, {} of them a row that existed before the work",
                HELD_CASES.load(Ordering::Relaxed),
                HELD_ROWS.load(Ordering::Relaxed),
                HELD_BEFORE_SOME_ROWS.load(Ordering::Relaxed),
            );
        }
    }

    fn apply(sut: Sut, _: &Shape, op: Op) -> Sut {
        let sut = apply_op(sut, op);
        work_fired_once_with_its_value(&sut);
        sut
    }
}

/// Drives the same ops with no model comparison and checks only the idle
/// rule and quiescence, so a twin that breaks one is red for it here.
struct IdleOnly;

impl StateMachineTest for IdleOnly {
    type SystemUnderTest = Sut;
    type Reference = Ref;

    fn init_test(shape: &Shape) -> Sut {
        let spec = spec_of(shape);
        init_sut(&spec, Driver::unchecked(spec.clone()))
    }

    fn apply(sut: Sut, _: &Shape, op: Op) -> Sut {
        apply_op(sut, op)
    }
}

/// Runs `op` and checks the idle rule and quiescence, the checks both tests
/// share.
fn apply_op(mut sut: Sut, op: Op) -> Sut {
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
                sut.newest = Some(id);
            }
        }
        Op::Undo(id) => undo_and_count(&mut sut, id),
        Op::UndoNewest => {
            if let Some(id) = sut.newest {
                undo_and_count(&mut sut, id);
            }
        }
        Op::CompleteDone => {
            if let Some(OccurrenceId::Work { answer, .. }) = d
                .pending
                .front()
                .filter(|req| {
                    &*req.transition == "Accept"
                        && d.exec()
                            .tokens()
                            .token("Accepted", req.subject.as_str())
                            .is_some()
                })
                .map(|req| req.occurrence.clone())
            {
                sut.overwrote.insert(answer.to_string());
            }
            // The drawn `Consume` has an empty input: only the work reads `n`.
            d.complete_oldest(|req| {
                if &*req.transition != "Accept" {
                    return EffectResult::Done(Vec::new());
                }
                let n = Value::Integer(int(&req.input, "n"));
                EffectResult::Done(vec![
                    Output::new("Accepted", &req.subject, content(&[("n", n.clone())])),
                    Output::new("Out", &req.subject, content(&[("n", n)])),
                ])
            })
            .expect("the net quiesces");
        }
        Op::CompleteFailed => d
            .complete_oldest(|_| EffectResult::Failed(Cause::definite("rejected")))
            .expect("the net quiesces"),
        Op::CompleteUnknown => d
            .complete_oldest(|_| EffectResult::Unknown(Cause::definite("reset")))
            .expect("the net quiesces"),
    }
    offered_asks_have_their_answer_enabled(&sut.d, &op_text);
    no_auto_transition_is_enabled(&sut, &op_text);
    sut
}

prop_state_machine! {
    #![proptest_config(ProptestConfig {
        cases: CASES_RUN,
        .. ProptestConfig::default()
    })]

    #[test]
    fn answers_follow_the_model(sequential 1..30 => Test);

    #[test]
    fn idle_rule_and_quiescence_hold(sequential 1..30 => IdleOnly);
}

/// The held arm with `before = Some`: the parked work of `a0` holds the `Item`
/// row that the work of `a2` consumed, so the undo of `a2` leaves that row and
/// the reach counter must see it (#195). Needs no drawn shape: the holder is an
/// earlier answer's own work.
#[test]
fn a_held_row_the_work_consumed_is_counted() {
    let shape = Shape {
        consume_accepted: None,
    };
    let answer = |id| Op::Answer {
        s: "y",
        id,
        stale: false,
        value: None,
    };
    let mut sut = Test::init_test(&shape);
    for op in [
        Op::WriteItem { s: "y", n: 0 },
        answer("a0"),
        Op::WriteItem { s: "y", n: 0 },
        Op::CompleteUnknown,
        answer("a2"),
        Op::CompleteDone,
        Op::Undo("a2"),
    ] {
        sut = Test::apply(sut, &shape, op);
    }
    assert_eq!(sut.held.rows, 1, "the undo of a2 leaves the held Item row");
    assert_eq!(
        sut.held.before_some_rows, 1,
        "the held Item row existed before the work took it: before = Some"
    );
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

/// A failed work makes its subject busy on its own: a dropped unknown
/// outcome consumes `Answered.Accept` and leaves `WorkFailed.Accept`.
#[test]
fn no_ask_is_offered_while_its_work_failed() {
    let mut d = Driver::new(answer_spec());
    d.write("Item", "x", content(&[("n", Value::Integer(1))]))
        .expect("the net quiesces");
    let c = answer_call(&d, "x", "a0", false, None);
    d.answer(c).expect("the ask applies");
    let (_, _, _, occurrence) = d.exec().in_flight().next().expect("the answered work runs");
    let occurrence = occurrence.clone();
    d.complete_oldest(|_| EffectResult::Unknown(Cause::definite("reset")))
        .expect("the net quiesces");
    d.resolve(&occurrence, Resolution::Drop)
        .expect("the net quiesces");
    d.write("Item", "x", content(&[("n", Value::Integer(2))]))
        .expect("the net quiesces");
    assert!(d.exec().tokens().token("WorkFailed.Accept", "x").is_some());
    assert!(d.exec().tokens().token("Answered.Accept", "x").is_none());
    offered_asks_have_their_answer_enabled(&d, "the drop of a0");
}
