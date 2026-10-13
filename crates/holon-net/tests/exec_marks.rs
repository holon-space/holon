//! Increment 2: a failure mark is keyed by the content of the read trace, and
//! a derived place is transparent in that trace (ADR 0032 §9 NET-mark.a).
//!
//! Twins: `HOLON_NET_TWIN=mark-by-version` keys the mark by row versions;
//! `HOLON_NET_TWIN=no-mark` sets no mark at all.

mod exec_support;

use exec_support::Driver;
use exec_support::content;
use exec_support::text;
use holon_api::Value;
use holon_net::exec::Body;
use holon_net::exec::Cause;
use holon_net::exec::Class;
use holon_net::exec::Content;
use holon_net::exec::NetDef;
use holon_net::exec::Output;
use holon_net::exec::Settled;
use holon_net::exec::TransitionDef;
use holon_net::exec::Trigger;
use proptest::prelude::*;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;
use proptest_state_machine::prop_state_machine;

const TASK: &str = "t";

/// `Check` reads only `Upper`, a derived place over `Task.title`; it never
/// reads `Task.note`.
fn net() -> NetDef {
    NetDef::default()
        .place("Task", Class::Process)
        .place("Checked", Class::Process)
        .derived("Upper", |r, s| {
            let title = r.text("Task", s, "title")?;
            Some(content(&[("upper", text(&title.to_uppercase()))]))
        })
        .transition(
            TransitionDef::new(
                "Check",
                Trigger::Auto,
                |r, s| {
                    let upper = r.text("Upper", s, "upper")?;
                    (upper != "SKIP").then(|| content(&[("upper", text(&upper))]))
                },
                Body::Pure(|s, input| match &input["upper"] {
                    Value::String(u) if u.starts_with("BAD") => {
                        Err(Cause::definite(format!("'{u}' does not pass")))
                    }
                    _ => Ok(vec![Output::new("Checked", s, input.clone())]),
                }),
            )
            .reads("Task")
            .inhibits("Checked"),
        )
}

#[derive(Clone, Debug)]
enum Op {
    SetTitle(&'static str),
    SetNote(&'static str),
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
            prop::sample::select(vec!["bad1", "bad2", "ok", "skip"]).prop_map(Op::SetTitle),
            prop::sample::select(vec!["x", "y"]).prop_map(Op::SetNote),
        ]
        .boxed()
    }

    fn apply(state: (), _: &Op) {
        state
    }
}

struct Sut {
    d: Driver,
    title: &'static str,
    note: &'static str,
    /// The title the last failure of `Check` saw.
    last_failed: Option<&'static str>,
}

fn failures(d: &Driver) -> usize {
    d.exec
        .occurrences()
        .filter(|(_, s)| matches!(s, Some(Settled::Failed(_))))
        .count()
}

struct Test;

impl StateMachineTest for Test {
    type SystemUnderTest = Sut;
    type Reference = Ref;

    fn init_test(_: &()) -> Sut {
        Sut {
            d: Driver::new(net()),
            title: "",
            note: "",
            last_failed: None,
        }
    }

    fn apply(mut sut: Sut, _: &(), op: Op) -> Sut {
        match op {
            Op::SetTitle(t) => sut.title = t,
            Op::SetNote(n) => sut.note = n,
        }
        let before = failures(&sut.d);
        let task: Content = content(&[("title", text(sut.title)), ("note", text(sut.note))]);
        sut.d.exec.write("Task", TASK, task);
        let stepped = sut.d.step();
        assert!(
            stepped.is_ok(),
            "step_reaches_quiescence: a failing firing re-fired without bound: {stepped:?}"
        );
        if failures(&sut.d) > before {
            assert!(
                sut.last_failed != Some(sut.title),
                "unread_edit_never_retries: Check failed again on title '{}' after an edit \
                 that changed no field it reads",
                sut.title
            );
            sut.last_failed = Some(sut.title);
        }
        sut
    }
}

prop_state_machine! {
    #[test]
    fn marks_are_keyed_by_read_content(sequential 1..30 => Test);
}
