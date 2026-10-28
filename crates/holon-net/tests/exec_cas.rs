//! Increment 3: the record compares and sets each read row by its version,
//! and a stale effect lands in a candidate place, applies on a data-class
//! place, or is withheld disclosed (ADR 0032 §9 NET-fire.a, NET-cand.a).
//!
//! Twin: `HOLON_NET_TWIN=no-cas` records without the compare.

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
use holon_net::exec::Capacity;
use holon_net::exec::Class;
use holon_net::exec::Content;
use holon_net::exec::EffectRequest;
use holon_net::exec::EffectResult;
use holon_net::exec::Output;
use holon_net::exec::Receiver;
use holon_net::exec::Route;
use holon_net::exec::Settled;
use holon_net::exec::Trigger;
use proptest::prelude::*;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;
use proptest_state_machine::prop_state_machine;

use crate::exec_support;

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
        .fifo("DocA.Candidate", Class::Process)
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
                d.write(place, DOC, content(&[("title", text(title))]))
                    .expect("the net quiesces");
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
    d.write("DocC", DOC, content(&[("title", text("Foo"))]))
        .expect("the net quiesces");
    d.write("DocC", DOC, content(&[("title", text("Bar"))]))
        .expect("the net quiesces");
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
    assert_eq!(
        stale_routes(&d),
        vec![("DocC", Route::Applied)],
        "the apply over the edit is disclosed"
    );
}

/// The routes of the one occurrence that settled over changed inputs.
fn stale_routes(d: &Driver) -> Vec<(&'static str, Route)> {
    let routes: Vec<_> = d
        .exec()
        .occurrences()
        .filter_map(|(_, s)| match s {
            Some(Settled::OverChangedInputs { routes, .. }) => Some(routes.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(routes.len(), 1, "one stale settle: {routes:?}");
    routes[0]
        .iter()
        .map(|r| {
            let place = DOCS
                .into_iter()
                .find(|p| r.place.as_str() == *p)
                .expect("a doc place");
            (place, r.route.clone())
        })
        .collect()
}

/// One stale effect writes a data place and a process place with no
/// candidate: the settle discloses both the apply and the withhold.
#[test]
fn a_mixed_route_stale_effect_discloses_every_route() {
    let mut d = Driver::new(net());
    d.write("DocC", DOC, content(&[("title", text("Foo"))]))
        .expect("the net quiesces");
    d.write("DocC", DOC, content(&[("title", text("Bar"))]))
        .expect("the net quiesces");
    d.complete_oldest(|req| {
        let EffectResult::Done(mut outputs) = lowered(req) else {
            unreachable!("lowered is Done")
        };
        outputs.push(Output::new(
            "DocB",
            &req.subject,
            content(&[("title", text("x"))]),
        ));
        EffectResult::Done(outputs)
    })
    .expect("the net quiesces");

    assert_eq!(
        stale_routes(&d),
        vec![("DocC", Route::Applied), ("DocB", Route::Withheld)]
    );
    assert!(
        d.exec().tokens().token("DocB", DOC).is_none(),
        "the withheld row is not written"
    );
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
                Body::pure(|s, input| Ok(vec![Output::new("Mid", s, input.clone())])),
            )
            .reads("Src"),
        )
        .transition(
            Transition::new(
                "Double",
                Trigger::Auto,
                bind!(|r, s| {
                    let src = Value::Integer(n_of(r, "Src", s)?);
                    let doubled = Value::Integer(2 * n_of(r, "Mid", s)?);
                    (r.field("Out", s, "src") != Some(src.clone()))
                        .then(|| content(&[("n", doubled), ("src", src)]))
                }),
                Body::pure(|s, input| Ok(vec![Output::new("Out", s, input.clone())])),
            )
            .reads("Src"),
        );
    let mut d = Driver::new(net);
    d.write("Src", "x", n(1)).expect("the net quiesces");
    d.write("Src", "x", n(2)).expect("the net quiesces");

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
        exec_support::load(spec.netdef()),
        Err(holon_net::exec::LoadError::ConsumesData { .. })
    ));
}

/// REV11-running.a: a firing holds what it consumes, so an outside write into
/// a consumed place cannot make that firing stale. Only READ inputs can be
/// stale. Without this, the second write makes `Apply` stale and its store
/// token — a process place with no candidate — is withheld and lost (#322 E1,
/// the cause of #315's D1).
#[test]
fn a_consumed_input_is_never_stale() {
    let n = |v: i64| content(&[("n", Value::Integer(v))]);
    let net = Spec::default()
        .place("Queue", Class::Process)
        .place("Store", Class::Process)
        .transition(
            Transition::new(
                "Apply",
                Trigger::Auto,
                bind!(|r, s| {
                    let seq = n_of(r, "Queue", s)?;
                    n_of(r, "Store", s)?;
                    Some(content(&[("n", Value::Integer(seq))]))
                }),
                Body::Effectful(Receiver::None),
            )
            .consumes("Queue")
            .consumes("Store"),
        );
    let mut d = Driver::new(net);
    d.write("Store", DOC, n(0)).expect("the net quiesces");
    d.write("Queue", DOC, n(1)).expect("the net quiesces");
    assert_eq!(d.pending.len(), 1, "the write holds both tokens");
    // A second write while the first is held: `Queue`'s version moves.
    d.write("Queue", DOC, n(2)).expect("the net quiesces");

    let settle = |d: &mut Driver| {
        d.complete_oldest(|req| {
            EffectResult::Done(vec![Output::new(
                "Store",
                &req.subject,
                content(&[("n", Value::Integer(int(&req.input, "n")))]),
            )])
        })
        .expect("the net quiesces")
    };
    settle(&mut d);
    // The returned store token took the second queue entry at once, so the
    // primary is in that firing's hold.
    assert_eq!(
        d.pending.len(),
        1,
        "lost_primary: 'Apply' held 'Store', yet the changed 'Queue' withheld it"
    );
    settle(&mut d);

    assert_eq!(
        int(
            &d.exec()
                .tokens()
                .token("Store", DOC)
                .expect("a store")
                .content,
            "n"
        ),
        2,
        "both writes reached the store"
    );
    assert!(
        !d.exec()
            .occurrences()
            .any(|(_, s)| matches!(s, Some(Settled::OverChangedInputs { .. }))),
        "a consumed input was treated as stale"
    );
}

/// REV2-cand.c: a candidate place holds a FIFO, so the second withheld
/// normalization waits behind the first instead of replacing it. Without it
/// the waiting token is dropped (#322 E4, #315's finding 3).
#[test]
fn a_put_never_drops_a_waiting_candidate() {
    let mut d = Driver::new(net());
    // Each firing is stale — an edit lands while its effect runs — so its
    // output is kept in the candidate place instead of applied.
    for (read, edit) in [("Foo", "Bar"), ("Bar", "Qux")] {
        d.write("DocA", DOC, content(&[("title", text(read))]))
            .expect("the net quiesces");
        assert_eq!(d.pending.len(), 1, "'{read}' is unnormalized");
        d.write("DocA", DOC, content(&[("title", text(edit))]))
            .expect("the net quiesces");
        d.complete_oldest(lowered).expect("the net quiesces");
    }
    let kept: Vec<String> = d
        .exec()
        .tokens()
        .contents("DocA.Candidate", DOC)
        .iter()
        .map(|c| match c.get("output") {
            Some(Value::Object(o)) => match o.get("title") {
                Some(Value::String(t)) => t.clone(),
                other => panic!("a kept output has title {other:?}"),
            },
            other => panic!("a candidate token has output {other:?}"),
        })
        .collect();
    assert_eq!(
        kept,
        vec!["foo".to_string(), "bar".to_string()],
        "both withheld normalizations wait, oldest first"
    );
}

/// REV2-cand.b: a put with no compare-and-set behind it never replaces. A
/// fact commits unconditionally (NET-fire.a), so the second fact would drop
/// the first one still waiting in the single-token place; the executor
/// refuses loudly instead (#322 E3).
#[test]
#[should_panic(expected = "would drop the held token")]
fn a_blind_put_into_an_occupied_single_token_place_refuses() {
    let net = Spec::default()
        .place("In", Class::Process)
        .place("Note", Class::Process)
        .transition(
            Transition::new(
                "Work",
                Trigger::Auto,
                bind!(|r, s| Some(content(&[("n", Value::Integer(n_of(r, "In", s)?))]))),
                Body::Effectful(Receiver::None),
            )
            .consumes("In")
            .fact("Note"),
        );
    let mut d = Driver::new(net);
    for n in [1, 2] {
        d.write("In", DOC, content(&[("n", Value::Integer(n))]))
            .expect("the net quiesces");
        d.complete_oldest(|req| {
            EffectResult::Done(vec![Output::new(
                "Note",
                &req.subject,
                content(&[("n", Value::Integer(int(&req.input, "n")))]),
            )])
        })
        .expect("the net quiesces");
    }
}

/// A candidate place must hold every waiting token (REV2-cand.c), so the load
/// refuses one that is not declared FIFO.
#[test]
fn a_candidate_place_that_is_not_fifo_is_refused() {
    let spec = Spec::default()
        .place_with_candidate("DocA", "DocA.Candidate")
        .place("DocA.Candidate", Class::Process);
    assert!(matches!(
        exec_support::load(spec.netdef()),
        Err(holon_net::exec::LoadError::CandidateNotFifo { .. })
    ));
}

/// 322/E3.c: a fact into a row its firing read is a compare-and-set against
/// the token it read. Nothing moved `Base` while the effect ran, so the
/// rewrite lands — and it lands on the default `Single` capacity, because the
/// read arc, not the declaration, is what permits it.
#[test]
fn a_fact_rewrite_of_a_read_row_is_a_compare_and_set() {
    let mut d = Driver::new(fact_net(Capacity::Single, ReadArc::Base));
    rewrite_twice(&mut d);
    assert_eq!(base_n(&d), 2, "the second rewrite compared and set");
    assert!(
        !d.exec()
            .occurrences()
            .any(|(_, s)| matches!(s, Some(Settled::OverChangedInputs { .. }))),
        "no firing read a row that moved"
    );
}

/// 322/E3.c, the case of 322/F3: the row the firing read moved while the
/// effect ran, so its fact is NOT put. The newer token wins, the occurrence
/// settles `OverChangedInputs` and discloses the withheld fact, and the
/// changed read input enables `Work` again.
#[test]
fn a_fact_into_a_read_row_that_moved_loses_to_the_newer_token() {
    let mut d = Driver::new(fact_net(Capacity::Register, ReadArc::Base));
    d.write("Base", DOC, content(&[("n", Value::Integer(0))]))
        .expect("the net quiesces");
    d.write("In", DOC, content(&[("n", Value::Integer(1))]))
        .expect("the net quiesces");
    d.write("Base", DOC, content(&[("n", Value::Integer(9))]))
        .expect("the net quiesces");
    d.complete_oldest(|req| {
        EffectResult::Done(vec![Output::new(
            "Base",
            &req.subject,
            content(&[("n", Value::Integer(int(&req.input, "n")))]),
        )])
    })
    .expect("the net quiesces");
    assert_eq!(
        base_n(&d),
        9,
        "the token written while the effect ran stands"
    );
    let routes: Vec<_> = d
        .exec()
        .occurrences()
        .filter_map(|(_, s)| match s {
            Some(Settled::OverChangedInputs { routes, .. }) => Some(routes.clone()),
            _ => None,
        })
        .flatten()
        .map(|r| (r.place.as_str().to_string(), r.route))
        .collect();
    assert_eq!(
        routes,
        vec![("Base".to_string(), Route::Withheld)],
        "the withheld fact is disclosed"
    );
}

/// 322/E3.b: a fact into a row its firing did NOT read carries no comparison,
/// so the place's declaration alone permits it. A `Register` is the row the
/// writing transitions own, and the write-back net declares `Base` and
/// `OnDisk` registers for the legs that rewrite them.
#[test]
fn a_fact_rewrite_of_an_unread_register_place_is_allowed() {
    let mut d = Driver::new(fact_net(Capacity::Register, ReadArc::Nothing));
    rewrite_twice(&mut d);
    assert_eq!(base_n(&d), 2, "the fact rewrote the register");
}

/// 322/E3.b: `Single` is the default and keeps the loud refusal of 322/E3.a.
/// The same net as the register case, one word of declaration apart: nothing
/// compared the token this fact replaces, so the executor refuses.
#[test]
#[should_panic(expected = "would drop the held token")]
fn a_fact_rewrite_of_an_unread_single_place_refuses() {
    let mut d = Driver::new(fact_net(Capacity::Single, ReadArc::Nothing));
    rewrite_twice(&mut d);
}

fn base_n(d: &Driver) -> i64 {
    int(
        &d.exec()
            .tokens()
            .token("Base", DOC)
            .expect("a base")
            .content,
        "n",
    )
}

/// Seeds `Base` from outside the net, then lets `Work` fact a new content
/// over it twice.
fn rewrite_twice(d: &mut Driver) {
    d.write("Base", DOC, content(&[("n", Value::Integer(0))]))
        .expect("the net quiesces");
    for n in [1, 2] {
        d.write("In", DOC, content(&[("n", Value::Integer(n))]))
            .expect("the net quiesces");
        d.complete_oldest(|req| {
            EffectResult::Done(vec![Output::new(
                "Base",
                &req.subject,
                content(&[("n", Value::Integer(int(&req.input, "n")))]),
            )])
        })
        .expect("the net quiesces");
    }
}

/// Whether `Work` reads the row it facts: the arc that makes the fact a
/// compare-and-set (322/E3.c) instead of a declared replacement (322/E3.b).
enum ReadArc {
    Base,
    Nothing,
}

/// `Work` consumes `In` and facts `Base`, which carries `capacity`.
fn fact_net(capacity: Capacity, reads: ReadArc) -> Spec {
    let spec = Spec::default().place("In", Class::Process);
    let spec = match capacity {
        Capacity::Register => spec.register("Base", Class::Process),
        Capacity::Single => spec.place("Base", Class::Process),
        Capacity::Fifo => panic!("a FIFO base is a different test"),
    };
    let work = Transition::new(
        "Work",
        Trigger::Auto,
        bind!(|r, s| Some(content(&[("n", Value::Integer(n_of(r, "In", s)?))]))),
        Body::Effectful(Receiver::None),
    )
    .consumes("In")
    .fact("Base");
    spec.transition(match reads {
        ReadArc::Base => work.reads("Base"),
        ReadArc::Nothing => work,
    })
}
