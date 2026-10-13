//! Increment 3: the record compares and sets each read row by its version,
//! and a stale effect lands in a candidate place or fails disclosed
//! (ADR 0032 §9 NET-fire.a, NET-cand.a).
//!
//! Twin: `HOLON_NET_TWIN=no-cas` records without the compare.

mod exec_support;

use std::collections::BTreeMap;

use exec_support::Driver;
use exec_support::content;
use exec_support::int;
use exec_support::string;
use exec_support::text;
use holon_api::Value;
use holon_net::exec::Body;
use holon_net::exec::Class;
use holon_net::exec::EffectResult;
use holon_net::exec::NetDef;
use holon_net::exec::Output;
use holon_net::exec::Reader;
use holon_net::exec::Receiver;
use holon_net::exec::Settled;
use holon_net::exec::Subject;
use holon_net::exec::TransitionDef;
use holon_net::exec::Trigger;
use proptest::prelude::*;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;
use proptest_state_machine::prop_state_machine;

const DOC: &str = "d";

fn normalize(place: &'static str) -> TransitionDef {
    let bind = match place {
        "DocA" => |r: &mut Reader<'_>, s: &Subject| unnormalized(r, "DocA", s),
        "DocB" => |r: &mut Reader<'_>, s: &Subject| unnormalized(r, "DocB", s),
        other => panic!("no doc place {other}"),
    };
    TransitionDef::new(
        &format!("Normalize{place}"),
        Trigger::Auto,
        bind,
        Body::Effectful(Receiver::None),
    )
    .reads(place)
}

fn unnormalized(r: &mut Reader<'_>, place: &str, s: &Subject) -> Option<holon_net::exec::Content> {
    let title = r.text(place, s, "title")?;
    (title != title.to_lowercase()).then(|| content(&[("title", text(&title))]))
}

/// `DocA` declares an apply transition fed by `DocA.Candidate`; `DocB` does
/// not.
fn net() -> NetDef {
    NetDef::default()
        .place_with_candidate("DocA", "DocA.Candidate")
        .place("DocA.Candidate", Class::Process)
        .place("DocB", Class::Process)
        .transition(normalize("DocA"))
        .transition(normalize("DocB"))
}

const DOCS: [&str; 2] = ["DocA", "DocB"];

#[derive(Clone, Debug)]
enum Op {
    Edit(&'static str, &'static str),
    EffectDone,
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
            (
                prop::sample::select(DOCS.to_vec()),
                prop::sample::select(vec!["Foo", "Bar", "baz"])
            )
                .prop_map(|(p, t)| Op::Edit(p, t)),
            Just(Op::EffectDone),
        ]
        .boxed()
    }

    fn apply(state: (), _: &Op) {
        state
    }
}

struct Sut {
    d: Driver,
    /// The last title the user wrote, per doc place.
    edits: BTreeMap<&'static str, &'static str>,
}

struct Test;

impl StateMachineTest for Test {
    type SystemUnderTest = Sut;
    type Reference = Ref;

    fn init_test(_: &()) -> Sut {
        Sut {
            d: Driver::new(net()),
            edits: BTreeMap::new(),
        }
    }

    fn apply(mut sut: Sut, _: &(), op: Op) -> Sut {
        match op {
            Op::Edit(place, title) => {
                sut.edits.insert(place, title);
                sut.d
                    .exec
                    .write(place, DOC, content(&[("title", text(title))]));
                sut.d.step().expect("the net quiesces");
            }
            Op::EffectDone => sut
                .d
                .complete_oldest(|req| {
                    let place = req.transition.trim_start_matches("Normalize");
                    let lower = string(&req.input, "title").to_lowercase();
                    EffectResult::Done(vec![Output::new(
                        place,
                        &req.subject,
                        content(&[("title", text(&lower))]),
                    )])
                })
                .expect("the net quiesces"),
        }
        sut
    }

    fn check_invariants(sut: &Sut, _: &()) {
        for (place, edit) in &sut.edits {
            let stored = string(
                &sut.d
                    .exec
                    .tokens()
                    .token(place, DOC)
                    .expect("edited")
                    .content,
                "title",
            );
            assert!(
                stored == *edit || stored == edit.to_lowercase(),
                "no_edit_lost: {place} holds '{stored}' but the user last wrote '{edit}'"
            );
        }
    }
}

prop_state_machine! {
    #[test]
    fn the_record_never_clobbers_an_edit(sequential 1..30 => Test);
}

fn n_of(r: &mut Reader<'_>, place: &str, s: &Subject) -> Option<i64> {
    match r.field(place, s, "n")? {
        Value::Integer(n) => Some(n),
        other => panic!("n is {other:?}"),
    }
}

/// Two pure firings decided in one round: `Mirror` writes the `Mid` row that
/// `Double` read, so `Double` is stale, rolls back and fires again.
#[test]
fn a_stale_pure_firing_rolls_back_and_refires() {
    let n = |v: i64| content(&[("n", Value::Integer(v))]);
    let net = NetDef::default()
        .place("Src", Class::Process)
        .place("Mid", Class::Process)
        .place("Out", Class::Process)
        .transition(
            TransitionDef::new(
                "Mirror",
                Trigger::Auto,
                |r, s| {
                    let src = n_of(r, "Src", s)?;
                    (n_of(r, "Mid", s) != Some(src)).then(|| content(&[("n", Value::Integer(src))]))
                },
                Body::Pure(|s, input| Ok(vec![Output::new("Mid", s, input.clone())])),
            )
            .reads("Src"),
        )
        .transition(
            TransitionDef::new(
                "Double",
                Trigger::Auto,
                |r, s| {
                    let doubled = 2 * n_of(r, "Mid", s)?;
                    (n_of(r, "Out", s) != Some(doubled))
                        .then(|| content(&[("n", Value::Integer(doubled))]))
                },
                Body::Pure(|s, input| Ok(vec![Output::new("Out", s, input.clone())])),
            )
            .reads("Src"),
        );
    let mut d = Driver::new(net);
    d.exec.write("Src", "x", n(1));
    d.step().expect("the net quiesces");
    d.exec.write("Src", "x", n(2));
    d.exec.write("Out", "x", n(99));
    d.step().expect("the net quiesces");

    let out = int(
        &d.exec.tokens().token("Out", "x").expect("doubled").content,
        "n",
    );
    assert_eq!(out, 4, "Out follows the final Mid");
    let stale = d
        .exec
        .occurrences()
        .filter(|(_, s)| matches!(s, Some(Settled::Failed(c)) if c.text.starts_with("stale")))
        .count();
    assert_eq!(
        stale, 1,
        "stale_pure_rolls_back: Double read Mid before Mirror wrote it in the same round"
    );
}
