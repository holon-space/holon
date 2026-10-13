//! Increment 3: the record compares and sets each read row by its version,
//! and a stale effect lands in a candidate place, applies on a data-class
//! place, or is withheld disclosed (ADR 0032 §9 NET-fire.a, NET-cand.a).
//!
//! Twin: `HOLON_NET_TWIN=no-cas` records without the compare.

mod exec_support;

use exec_support::Driver;
use exec_support::Reads;
use exec_support::Spec;
use exec_support::Transition;
use exec_support::bind;
use exec_support::content;
use exec_support::int;
use exec_support::string;
use exec_support::text;
use holon_api::Value;
use holon_net::exec::Body;
use holon_net::exec::Class;
use holon_net::exec::Content;
use holon_net::exec::EffectRequest;
use holon_net::exec::EffectResult;
use holon_net::exec::Output;
use holon_net::exec::Receiver;
use holon_net::exec::Settled;
use holon_net::exec::Trigger;
use proptest::prelude::*;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;
use proptest_state_machine::prop_state_machine;

const DOC: &str = "d";

fn normalize(place: &'static str) -> Transition {
    let bind = match place {
        "DocA" => bind!(|r, s| unnormalized(r, "DocA", s)),
        "DocB" => bind!(|r, s| unnormalized(r, "DocB", s)),
        "DocC" => bind!(|r, s| unnormalized(r, "DocC", s)),
        other => panic!("no doc place {other}"),
    };
    Transition::new(
        &format!("Normalize{place}"),
        Trigger::Auto,
        bind,
        Body::Effectful(Receiver::None),
    )
    .reads(place)
}

fn unnormalized(r: &mut dyn Reads, place: &str, s: &str) -> Option<Content> {
    let title = r.text(place, s, "title")?;
    (title != title.to_lowercase()).then(|| content(&[("title", text(&title))]))
}

/// `DocA` declares an apply transition fed by `DocA.Candidate`; `DocB` does
/// not; `DocC` is data class.
fn net() -> Spec {
    Spec::default()
        .place_with_candidate("DocA", "DocA.Candidate")
        .place("DocA.Candidate", Class::Process)
        .place("DocB", Class::Process)
        .place("DocC", Class::Data)
        .transition(normalize("DocA"))
        .transition(normalize("DocB"))
        .transition(normalize("DocC"))
}

const DOCS: [&str; 3] = ["DocA", "DocB", "DocC"];

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

fn lowered(req: &EffectRequest) -> EffectResult {
    let place = req.transition.trim_start_matches("Normalize");
    let lower = string(&req.input, "title").to_lowercase();
    EffectResult::Done(vec![Output::new(
        place,
        &req.subject,
        content(&[("title", text(&lower))]),
    )])
}

struct Test;

impl StateMachineTest for Test {
    type SystemUnderTest = Driver;
    type Reference = Ref;

    fn init_test(_: &()) -> Driver {
        Driver::new(net())
    }

    fn apply(mut d: Driver, _: &(), op: Op) -> Driver {
        match op {
            Op::Edit(place, title) => {
                d.write(place, DOC, content(&[("title", text(title))]));
                d.step().expect("the net quiesces");
            }
            Op::EffectDone => d.complete_oldest(lowered).expect("the net quiesces"),
        }
        d
    }
}

prop_state_machine! {
    #[test]
    fn the_record_never_clobbers_an_edit(sequential 1..30 => Test);
}

/// The data-class stand-in: an edit lands while the effect runs, and the
/// effect's output applies over it, disclosed.
#[test]
fn a_stale_effect_on_a_data_place_applies_and_discloses() {
    let mut d = Driver::new(net());
    d.write("DocC", DOC, content(&[("title", text("Foo"))]));
    d.step().expect("the net quiesces");
    d.write("DocC", DOC, content(&[("title", text("Bar"))]));
    d.complete_oldest(lowered).expect("the net quiesces");

    let stored = string(
        &d.exec()
            .tokens()
            .token("DocC", DOC)
            .expect("edited")
            .content,
        "title",
    );
    assert_eq!(stored, "foo", "the effect happened, so its output applies");
    let applied = d
        .exec()
        .occurrences()
        .filter(|(_, s)| matches!(s, Some(Settled::AppliedOverChangedInputs(_))))
        .count();
    assert_eq!(applied, 1, "the apply over the edit is disclosed");
}

fn n_of(r: &mut dyn Reads, place: &str, s: &str) -> Option<i64> {
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
    let net = Spec::default()
        .place("Src", Class::Process)
        .place("Mid", Class::Process)
        .place("Out", Class::Process)
        .transition(
            Transition::new(
                "Mirror",
                Trigger::Auto,
                bind!(|r, s| {
                    let src = n_of(r, "Src", s)?;
                    (n_of(r, "Mid", s) != Some(src)).then(|| content(&[("n", Value::Integer(src))]))
                }),
                Body::Pure(|s, input| Ok(vec![Output::new("Mid", s, input.clone())])),
            )
            .reads("Src"),
        )
        .transition(
            Transition::new(
                "Double",
                Trigger::Auto,
                bind!(|r, s| {
                    let doubled = 2 * n_of(r, "Mid", s)?;
                    (n_of(r, "Out", s) != Some(doubled))
                        .then(|| content(&[("n", Value::Integer(doubled))]))
                }),
                Body::Pure(|s, input| Ok(vec![Output::new("Out", s, input.clone())])),
            )
            .reads("Src"),
        );
    let mut d = Driver::new(net);
    d.write("Src", "x", n(1));
    d.step().expect("the net quiesces");
    d.write("Src", "x", n(2));
    d.write("Out", "x", n(99));
    d.step().expect("the net quiesces");

    let out = int(
        &d.exec()
            .tokens()
            .token("Out", "x")
            .expect("doubled")
            .content,
        "n",
    );
    assert_eq!(out, 4, "Out follows the final Mid");
    let stale = d
        .exec()
        .occurrences()
        .filter(|(_, s)| matches!(s, Some(Settled::Failed(c)) if c.text.starts_with("stale")))
        .count();
    assert_eq!(
        stale, 1,
        "stale_pure_rolls_back: Double read Mid before Mirror wrote it in the same round"
    );
}

/// A take on a data-class place needs claims (#168), so the load refuses it.
#[test]
fn a_consume_arc_on_a_data_place_is_refused() {
    let spec = Spec::default().place("Doc", Class::Data).transition(
        Transition::new(
            "Take",
            Trigger::Auto,
            bind!(|_r, _s| Some(Content::new())),
            Body::Effectful(Receiver::None),
        )
        .consumes("Doc"),
    );
    assert!(matches!(
        holon_net::exec::Executor::new(spec.netdef()),
        Err(holon_net::exec::LoadError::ConsumesData { .. })
    ));
}
