//! A transition names each place in at most one of its input arcs, once. A
//! net whose `consumes`, `reads` or `inhibits` repeat a place, or share one,
//! is refused when it is built: from a file with the file, the line and the
//! fields, from the Rust builder with the transition and the fields. Every
//! other net loads and runs; the same place in two transitions is legal.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use holon_net::exec::Body;
use holon_net::exec::Class;
use holon_net::exec::Content;
use holon_net::exec::Executor;
use holon_net::exec::NetDef;
use holon_net::exec::Output;
use holon_net::exec::Reader;
use holon_net::exec::Subject;
use holon_net::exec::TransitionDef;
use holon_net::exec::Trigger;
use holon_net::load::Logic;
use holon_net::load::net_def;
use proptest::prelude::*;
use proptest::sample::select;
use proptest::strategy::ValueTree;

const PLACES: [&str; 4] = ["A", "B", "C", "D"];
const FIELDS: [&str; 3] = ["consumes", "reads", "inhibits"];
const PATH: &str = "nets/gen.yaml";

/// The `consumes`, `reads` and `inhibits` lists of one transition.
type Inputs = [Vec<&'static str>; 3];

/// One transition: its trigger and its input lists.
type Spec = (Trigger, Inputs);

fn inputs() -> impl Strategy<Value = Inputs> {
    let list = || prop::collection::vec(select(&PLACES[..]), 0..=3);
    let free = (list(), list(), list()).prop_map(|(c, r, i)| [c, r, i]);
    // Free lists repeat a place in most nets; these never do, so the nets
    // that must load and run are drawn as often as the ones refused.
    let distinct = (
        Just(PLACES.to_vec()).prop_shuffle(),
        0..=4usize,
        0..=4usize,
        0..=4usize,
    )
        .prop_map(|(places, a, b, c)| {
            let mut cuts = [a, b, c];
            cuts.sort();
            let [a, b, c] = cuts;
            [
                places[..a].to_vec(),
                places[a..b].to_vec(),
                places[b..c].to_vec(),
            ]
        });
    prop_oneof![free, distinct]
}

fn net() -> impl Strategy<Value = Vec<Spec>> {
    let trigger = prop_oneof![Just(Trigger::Auto), Just(Trigger::Manual)];
    prop::collection::vec((trigger, inputs()), 1..=3)
}

fn name(i: usize) -> String {
    format!("T{i}")
}

/// Where transition `i` puts its token. It also inhibits the place, so it
/// fires once per subject: an auto transition that writes nothing it reads
/// fires again every round and never quiesces.
fn fired(i: usize) -> String {
    format!("Fired{i}")
}

#[derive(Debug)]
enum Refusal {
    /// The transition, the field of the first naming, the field of the
    /// second, the place.
    Twice(String, &'static str, &'static str, &'static str),
    /// An auto transition with no consume or read arc to find its subject by.
    NoSubject(String),
}

fn repeat(lists: &Inputs) -> Option<(&'static str, &'static str, &'static str)> {
    let named: Vec<(&'static str, &'static str)> = FIELDS
        .iter()
        .zip(lists)
        .flat_map(|(f, ps)| ps.iter().map(move |p| (*f, *p)))
        .collect();
    (1..named.len()).find_map(|later| {
        (0..later)
            .find(|&e| named[e].1 == named[later].1)
            .map(|e| (named[e].0, named[later].0, named[later].1))
    })
}

fn twice(t: usize, lists: &Inputs) -> Option<Refusal> {
    repeat(lists).map(|(first, second, place)| Refusal::Twice(name(t), first, second, place))
}

fn no_subject(t: usize, (trigger, [consumes, reads, _]): &Spec) -> Option<Refusal> {
    (*trigger == Trigger::Auto && consumes.is_empty() && reads.is_empty())
        .then(|| Refusal::NoSubject(name(t)))
}

/// `NetDef::check` takes the transitions one at a time, each checked for a
/// repeat before its subject.
fn built_refusal(net: &[Spec]) -> Option<Refusal> {
    net.iter()
        .enumerate()
        .find_map(|(t, spec)| twice(t, &spec.1).or_else(|| no_subject(t, spec)))
}

/// The file parser refuses a repeat in any transition before `Executor::new`
/// looks for a subject.
fn file_refusal(net: &[Spec]) -> Option<Refusal> {
    let mut specs = net.iter().enumerate();
    specs
        .clone()
        .find_map(|(t, (_, lists))| twice(t, lists))
        .or_else(|| specs.find_map(|(t, spec)| no_subject(t, spec)))
}

fn no_subject_refusal(transition: &str) -> String {
    format!(
        "Executor::new refused: auto transition '{transition}' has no consume or read arc to find its subjects by"
    )
}

/// The file, and the 1-based line each transition is declared on.
fn yaml(net: &[Spec]) -> (String, Vec<usize>) {
    let mut lines = vec!["name: gen".to_owned(), "places:".to_owned()];
    for p in PLACES {
        lines.push(format!("  {p}:"));
        lines.push("    capacity: single".to_owned());
    }
    for t in 0..net.len() {
        lines.push(format!("  {}:", fired(t)));
        lines.push("    capacity: single".to_owned());
    }
    lines.push("transitions:".to_owned());
    let mut declared = vec![];
    for (t, (trigger, lists)) in net.iter().enumerate() {
        lines.push(format!("  {}:", name(t)));
        declared.push(lines.len());
        let trigger = match trigger {
            Trigger::Auto => "auto",
            Trigger::Manual => "manual",
        };
        lines.push(format!("    trigger: {trigger}"));
        lines.push("    priority: 0".to_owned());
        let [consumes, reads, inhibits] = lists
            .clone()
            .map(|ps| ps.iter().map(|p| p.to_string()).collect::<Vec<_>>());
        let inhibits = [inhibits, vec![fired(t)]].concat();
        let fields = [
            ("consumes", consumes),
            ("reads", reads),
            ("inhibits", inhibits),
            ("produces", vec![fired(t)]),
        ];
        for (field, places) in fields {
            if !places.is_empty() {
                lines.push(format!("    {field}:"));
                lines.extend(places.iter().map(|p| format!("    - {p}")));
            }
        }
    }
    (lines.join("\n") + "\n", declared)
}

fn bind() -> holon_net::exec::Read {
    Arc::new(|_: &mut Reader<'_>, _: &Subject| Some(Content::new()))
}

fn body(i: usize) -> holon_net::exec::PureBody {
    let into = fired(i);
    Arc::new(move |s: &Subject, _: &Content| Ok(vec![Output::new(&into, s, Content::new())]))
}

fn logic(transition: &str) -> Logic {
    let i = transition[1..].parse().expect("transition names are T<i>");
    Logic::pure(bind(), body(i))
}

fn built(net: &[Spec]) -> NetDef {
    let mut def = NetDef::default();
    for p in PLACES {
        def = def.place(p, Class::Process);
    }
    for t in 0..net.len() {
        def = def.place(&fired(t), Class::Process);
    }
    for (t, (trigger, [consumes, reads, inhibits])) in net.iter().enumerate() {
        let mut td = TransitionDef::new(&name(t), *trigger, bind(), Body::Pure(body(t)));
        for p in consumes {
            td = td.consumes(p);
        }
        for p in reads {
            td = td.reads(p);
        }
        for p in inhibits {
            td = td.inhibits(p);
        }
        def = def.transition(td.inhibits(&fired(t)));
    }
    def
}

/// Puts a token into every place, which fires whatever it enables.
fn run(def: NetDef) -> Result<(), String> {
    let mut exec = Executor::new(def).map_err(|e| format!("Executor::new refused: {e}"))?;
    for p in PLACES {
        let quiesced = exec
            .write(p, "s", Content::new(), Duration::ZERO)
            .map_err(|e| format!("write into {p}: {e:?}"))?;
        assert!(
            quiesced.requests.is_empty(),
            "a pure net requests no effect"
        );
    }
    Ok(())
}

/// `build` and `run`, with a panic turned into the error it is.
fn no_panic(f: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    std::panic::catch_unwind(AssertUnwindSafe(f))
        .unwrap_or_else(|_| Err("panicked; the panic message is above".to_owned()))
}

fn names_the_repeat(
    refusal: &str,
    (transition, first, second, place): &(String, &str, &str, &str),
) -> Result<(), TestCaseError> {
    for part in [transition.as_str(), first, second, place] {
        prop_assert!(
            refusal.contains(&format!("`{part}`")) || refusal.contains(&format!("'{part}'")),
            "the refusal does not name `{part}`: {refusal}"
        );
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn a_net_file_that_names_a_place_twice_is_refused_at_its_line(net in net()) {
        let (text, declared) = yaml(&net);
        let outcome = no_panic(|| {
            let def = net_def(PATH, &text, logic).map_err(|e| format!("load refused: {e}"))?;
            run(def)
        });
        match file_refusal(&net) {
            Some(Refusal::Twice(transition, first, second, place)) => {
                let refusal = outcome.expect_err("the loader accepted a repeated place");
                let t: usize = transition[1..].parse().expect("transition names are T<i>");
                let at = format!("load refused: {PATH}:{}: ", declared[t]);
                prop_assert!(refusal.starts_with(&at), "not refused at `{at}`: {refusal}\n{text}");
                names_the_repeat(&refusal, &(transition, first, second, place))?;
            }
            Some(Refusal::NoSubject(transition)) => {
                prop_assert_eq!(outcome, Err(no_subject_refusal(&transition)), "{}", text)
            }
            None => prop_assert_eq!(outcome, Ok(()), "a net of distinct inputs must load and run\n{}", text),
        }
    }

    #[test]
    fn a_built_net_that_names_a_place_twice_is_refused_by_its_check(net in net()) {
        let outcome = no_panic(|| run(built(&net)));
        match built_refusal(&net) {
            Some(Refusal::Twice(transition, first, second, place)) => {
                let refusal = outcome.expect_err("the builder net was accepted with a repeated place");
                prop_assert!(
                    refusal.starts_with("Executor::new refused: "),
                    "not refused by the check: {refusal}\n{net:?}"
                );
                names_the_repeat(&refusal, &(transition, first, second, place))?;
            }
            Some(Refusal::NoSubject(transition)) => {
                prop_assert_eq!(outcome, Err(no_subject_refusal(&transition)), "{:?}", net)
            }
            None => prop_assert_eq!(outcome, Ok(()), "a net of distinct inputs must load and run\n{:?}", net),
        }
    }
}

/// Both load paths refuse `net` for `cause`, the file one at its line.
fn refused_on_both_paths(net: &[Spec], cause: &str) {
    let (text, declared) = yaml(net);
    let file =
        no_panic(|| run(net_def(PATH, &text, logic).map_err(|e| format!("load refused: {e}"))?))
            .expect_err("the loader accepted the net");
    let at = format!("load refused: {PATH}:{}: ", declared[0]);
    assert!(file.starts_with(&at) && file.contains(cause), "{file}");
    let built = no_panic(|| run(built(net))).expect_err("the builder accepted the net");
    assert!(
        built.starts_with("Executor::new refused: ") && built.contains(cause),
        "{built}"
    );
}

/// A place consumed twice would pass `decide`, which one token satisfies for
/// both arcs, and leave `start` without a token for the second take.
#[test]
fn a_place_consumed_twice_never_reaches_a_firing() {
    refused_on_both_paths(
        &[(Trigger::Auto, [vec!["A", "A"], vec![], vec![]])],
        "`consumes` names the place `A` twice",
    );
}

/// A manual transition needs no consume or read arc, so its inhibits alone
/// can carry the repeat.
#[test]
fn a_manual_transition_that_inhibits_a_place_twice_is_refused() {
    refused_on_both_paths(
        &[(Trigger::Manual, [vec![], vec![], vec!["A", "A"]])],
        "`inhibits` names the place `A` twice",
    );
}

/// The drawn nets reach every outcome the properties tell apart, the manual
/// transition with a repeat in its inhibits alone among them.
#[test]
fn the_generator_draws_every_outcome() {
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    let strategy = net();
    let mut counts = [0usize; 4];
    for _ in 0..256 {
        let net = strategy.new_tree(&mut runner).unwrap().current();
        let manual_inhibits_only = |name: &str| {
            let (trigger, [c, r, _]) = &net[name[1..].parse::<usize>().unwrap()];
            *trigger == Trigger::Manual && c.is_empty() && r.is_empty()
        };
        counts[match built_refusal(&net) {
            None => 0,
            Some(Refusal::NoSubject(_)) => 1,
            Some(Refusal::Twice(t, ..)) if manual_inhibits_only(&t) => 2,
            Some(Refusal::Twice(..)) => 3,
        }] += 1;
    }
    println!(
        "loads: {}, no subject: {}, manual inhibits twice: {}, other repeat: {}",
        counts[0], counts[1], counts[2], counts[3]
    );
    assert!(counts.iter().all(|&n| n > 0), "{counts:?}");
}
