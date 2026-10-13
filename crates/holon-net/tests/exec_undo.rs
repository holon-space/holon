//! Undo of an answer (XQ2.a), the settled chain of an answer, and the ask
//! hash. Each case runs on the executor and the reference model.

mod exec_support;

use std::sync::Arc;

use exec_support::Driver;
use exec_support::Transition;
use exec_support::answer_call;
use exec_support::answer_spec;
use exec_support::bind;
use exec_support::content;
use exec_support::string;
use exec_support::text;
use holon_api::Value;
use holon_api::user_transition::AnswerId;
use holon_api::user_transition::AnswerSettlement;
use holon_api::user_transition::ExpireReason;
use holon_api::user_transition::UftError;
use holon_net::exec::Body;
use holon_net::exec::Cause;
use holon_net::exec::Class;
use holon_net::exec::Content;
use holon_net::exec::EffectResult;
use holon_net::exec::Executor;
use holon_net::exec::Holder;
use holon_net::exec::NetDef;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Output;
use holon_net::exec::Place;
use holon_net::exec::Receiver;
use holon_net::exec::Reversal;
use holon_net::exec::ReversalOutcome;
use holon_net::exec::Settled;
use holon_net::exec::Subject;
use holon_net::exec::Trigger;
use holon_net::exec::UndoError;
use holon_net::exec::Undone;
use holon_net::exec::ask_hash;
use holon_net::exec::hash_text;
use holon_net::exec::work_occurrence;

fn n(v: i64) -> Content {
    content(&[("n", Value::Integer(v))])
}

/// `Item/x` with `n = 1`, answered by `a0`; its work runs.
fn answered() -> Driver {
    let mut d = Driver::new(answer_spec());
    d.write("Item", "x", n(1));
    d.step().expect("the net quiesces");
    d.answer(answer_call(&d, "x", "a0", false, Some("Theirs")))
        .expect("the ask applies");
    d.step().expect("the net quiesces");
    d
}

/// The work of `a0` is done: it wrote `Accepted/x` and `Out/x`.
fn done() -> Driver {
    let mut d = answered();
    d.complete_oldest(|req| {
        EffectResult::Done(vec![
            Output::new("Accepted", &req.subject, n(1)),
            Output::new("Out", &req.subject, n(1)),
        ])
    })
    .expect("the net quiesces");
    d
}

fn a0() -> OccurrenceId {
    OccurrenceId::caller("a0")
}

fn reversal(place: &str, outcome: ReversalOutcome) -> Reversal {
    Reversal {
        place: Place::new(place),
        subject: Subject::new("x"),
        outcome,
    }
}

#[test]
fn undo_reverses_the_work_and_discloses_each_row() {
    let mut d = done();
    assert_eq!(d.exec().settled(&a0()), Ok(Some(Settled::Done)));
    let undone = d.undo(&a0()).expect("a done answer undoes");
    assert_eq!(
        undone,
        Undone {
            withdrawn: false,
            reversed: vec![
                reversal("Accepted", ReversalOutcome::Removed),
                reversal("Item", ReversalOutcome::Restored),
                reversal("Out", ReversalOutcome::NotProcessClass),
            ],
        }
    );
    assert_eq!(
        d.exec()
            .tokens()
            .token("Item", "x")
            .map(|t| t.content.clone()),
        Some(n(1))
    );
    assert!(d.exec().derive("Asks.Accept", &Subject::new("x")).is_some());
    assert_eq!(
        d.exec().answer_settlement(&AnswerId("a0".to_string())),
        Ok(AnswerSettlement::Undone)
    );
    assert_eq!(
        d.requests_of("Accept").count(),
        1,
        "the work fired again after undo"
    );
}

#[test]
fn undo_leaves_a_row_changed_since_the_work() {
    let mut d = done();
    d.write("Accepted", "x", n(9));
    let undone = d.undo(&a0()).expect("a done answer undoes");
    assert_eq!(
        undone.reversed[0],
        reversal("Accepted", ReversalOutcome::Changed)
    );
    assert_eq!(
        d.exec()
            .tokens()
            .token("Accepted", "x")
            .map(|t| t.content.clone()),
        Some(n(9))
    );
}

#[test]
fn undo_before_the_work_withdraws_the_answer() {
    let mut d = Driver::new(answer_spec());
    d.write("Item", "x", n(1));
    d.step().expect("the net quiesces");
    d.answer(answer_call(&d, "x", "a0", false, None))
        .expect("the ask applies");
    let undone = d.undo(&a0()).expect("a waiting answer undoes");
    assert_eq!(
        undone,
        Undone {
            withdrawn: true,
            reversed: Vec::new(),
        }
    );
    assert_eq!(d.requests_of("Accept").count(), 0);
    assert_eq!(
        d.exec().settled(&a0()),
        Ok(Some(Settled::Failed(Cause::definite(
            "undone before the work ran"
        ))))
    );
}

#[test]
fn undo_refuses_what_it_cannot_undo() {
    let mut d = answered();
    let work = work_occurrence("Accept", "a0");
    assert_eq!(d.undo(&a0()), Err(UndoError::WorkUnsettled(work.clone())));
    assert_eq!(d.undo(&work), Err(UndoError::NotAnAnswer(work)));
    let unknown = OccurrenceId::caller("nope");
    assert_eq!(d.undo(&unknown), Err(UndoError::UnknownOccurrence(unknown)));
    d.complete_oldest(|_| EffectResult::Failed(Cause::definite("rejected")))
        .expect("the net quiesces");
    d.undo(&a0()).expect("a failed answer undoes");
    assert_eq!(d.undo(&a0()), Err(UndoError::AlreadyUndone(a0())));
}

#[test]
fn settled_follows_the_work() {
    let mut d = answered();
    assert_eq!(d.exec().settled(&a0()), Ok(None));
    assert_eq!(
        d.exec().answer_settlement(&AnswerId("a0".to_string())),
        Ok(AnswerSettlement::Working {
            work: work_occurrence("Accept", "a0")
        })
    );
    d.complete_oldest(|_| EffectResult::Failed(Cause::definite("rejected")))
        .expect("the net quiesces");
    assert_eq!(
        d.exec().settled(&a0()),
        Ok(Some(Settled::Failed(Cause::definite("rejected"))))
    );
}

#[test]
fn an_expired_answer_no_longer_applies() {
    let mut d = Driver::new(answer_spec());
    d.write("Item", "x", n(1));
    d.step().expect("the net quiesces");
    d.answer(answer_call(&d, "x", "a0", false, None))
        .expect("the ask applies");
    d.write("Item", "x", n(2));
    d.step().expect("the net quiesces");
    assert_eq!(d.requests_of("Accept").count(), 0);
    assert_eq!(
        d.exec().settled(&a0()),
        Ok(Some(Settled::Failed(Cause::definite(
            UftError::NoLongerApplies.to_string()
        ))))
    );
    assert_eq!(
        d.exec().answer_settlement(&AnswerId("a0".to_string())),
        Ok(AnswerSettlement::Expired {
            reason: ExpireReason::SituationChanged
        })
    );
    assert_eq!(
        d.answer(answer_call(&d, "x", "a1", true, None))
            .map(|a| a.0),
        Ok("a1".to_string()),
        "a stale seen hash is accepted, then expires"
    );
}

#[test]
fn a_second_answer_while_one_waits_is_refused() {
    let mut d = Driver::new(answer_spec());
    d.write("Item", "x", n(1));
    d.step().expect("the net quiesces");
    d.answer(answer_call(&d, "x", "a0", false, None))
        .expect("the ask applies");
    assert_eq!(
        d.answer(answer_call(&d, "x", "a1", false, None)),
        Err(UftError::AlreadyAnswered)
    );
    assert_eq!(
        d.answer(answer_call(&d, "y", "a2", false, None)),
        Err(UftError::NoLongerApplies)
    );
}

/// A derived place that hashes what its read of `Item.n` traced, for the
/// transitions `Accept` and `Other`.
fn hashing_executor() -> Executor {
    let net = NetDef::default().place("Item", Class::Process).derived(
        "H",
        Arc::new(|r, s| {
            let (n, trace) = r.traced(|r| r.field("Item", s, "n"));
            n?;
            Some(content(&[
                ("accept", text(&hash_text(&ask_hash("Accept", s, &trace)))),
                ("other", text(&hash_text(&ask_hash("Other", s, &trace)))),
            ]))
        }),
    );
    Executor::new(net).expect("the net loads")
}

#[test]
fn ask_hash_covers_content_subject_and_transition_not_version() {
    let mut e = hashing_executor();
    let hash = |e: &Executor, s: &str| e.derive("H", &Subject::new(s)).expect("Item holds a token");
    e.write("Item", "x", n(1));
    e.write("Item", "y", n(1));
    let first = hash(&e, "x");
    e.write("Item", "x", n(2));
    let changed = hash(&e, "x");
    e.write("Item", "x", n(1));
    let back = hash(&e, "x");
    assert_eq!(first, back, "a new version with the same content");
    assert_ne!(string(&first, "accept"), string(&changed, "accept"));
    assert_ne!(string(&first, "accept"), string(&hash(&e, "y"), "accept"));
    assert_ne!(string(&first, "accept"), string(&first, "other"));
}

/// `answer_spec` with `Consume`, an effectful downstream transition that
/// consumes `Accepted`. The work of `a0` is done and `Consume` holds
/// `Accepted/x`.
fn downstream() -> Driver {
    let spec = answer_spec().transition(
        Transition::new(
            "Consume",
            Trigger::Auto,
            bind!(|r, s| r.present("Accepted", s).then(Content::new)),
            Body::Effectful(Receiver::None),
        )
        .consumes("Accepted"),
    );
    let mut d = Driver::new(spec);
    d.write("Item", "x", n(1));
    d.step().expect("the net quiesces");
    d.answer(answer_call(&d, "x", "a0", false, None))
        .expect("the ask applies");
    d.step().expect("the net quiesces");
    d.complete_oldest(|req| {
        EffectResult::Done(vec![
            Output::new("Accepted", &req.subject, n(1)),
            Output::new("Out", &req.subject, n(1)),
        ])
    })
    .expect("the net quiesces");
    assert_eq!(
        d.requests_of("Consume").count(),
        1,
        "Consume holds Accepted/x"
    );
    d
}

#[test]
fn undo_while_a_downstream_firing_holds_a_row() {
    let mut d = downstream();
    let undone = d.undo(&a0()).expect("a done answer undoes");
    assert_eq!(
        undone.reversed,
        vec![
            reversal(
                "Accepted",
                ReversalOutcome::Held(Holder::Running(Arc::from("Consume")))
            ),
            reversal("Item", ReversalOutcome::Restored),
            reversal("Out", ReversalOutcome::NotProcessClass),
        ]
    );
    d.complete_oldest(|_| EffectResult::Done(Vec::new()))
        .expect("the net quiesces");
    assert!(d.exec().tokens().token("Accepted", "x").is_none());
}

#[test]
fn settled_stays_done_after_an_undo() {
    let mut d = done();
    d.undo(&a0()).expect("a done answer undoes");
    assert_eq!(d.exec().settled(&a0()), Ok(Some(Settled::Done)));
    assert_eq!(
        d.exec().answer_settlement(&AnswerId("a0".to_string())),
        Ok(AnswerSettlement::Undone)
    );
}

#[test]
fn undo_of_an_expired_answer_keeps_no_longer_applies() {
    let mut d = Driver::new(answer_spec());
    d.write("Item", "x", n(1));
    d.step().expect("the net quiesces");
    d.answer(answer_call(&d, "x", "a0", false, None))
        .expect("the ask applies");
    d.write("Item", "x", n(2));
    d.step().expect("the net quiesces");
    let undone = d.undo(&a0()).expect("an expired answer undoes");
    assert_eq!(
        undone,
        Undone {
            withdrawn: false,
            reversed: Vec::new(),
        }
    );
    assert_eq!(
        d.exec().settled(&a0()),
        Ok(Some(Settled::Failed(Cause::definite(
            UftError::NoLongerApplies.to_string()
        ))))
    );
}
