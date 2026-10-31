//! Verifier probe: producers of several places, manual co-producers, and
//! several subjects enabled in the same round.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use holon_api::Value;
use holon_net::exec::Content;
use holon_net::exec::Executor;
use holon_net::exec::Output;
use holon_net::exec::Reader;
use holon_net::exec::Subject;
use holon_net::load::Logic;
use holon_net::load::net_def;
use proptest::prelude::*;

const SHARED: usize = 3;

#[derive(Debug, Clone)]
struct T {
    manual: bool,
    priority: i32,
    // indices into 0..SHARED, or SHARED meaning own place
    outs: BTreeSet<usize>,
    enabled: Vec<bool>,
}

fn place(i: usize, o: usize) -> String {
    if o == SHARED {
        format!("Own{i}")
    } else {
        format!("Out{o}")
    }
}

fn yaml(ts: &[T]) -> String {
    let mut places = String::from("  Start:\n    capacity: single\n  Go:\n    capacity: single\n");
    for g in 0..SHARED {
        places += &format!("  Out{g}:\n    capacity: single\n");
    }
    let mut transitions = String::from(
        "  Fan:\n    trigger: auto\n    priority: 0\n    consumes:\n    - Start\n    produces:\n    - Go\n",
    );
    for (i, t) in ts.iter().enumerate() {
        places += &format!("  In{i}:\n    capacity: single\n");
        if t.outs.contains(&SHARED) {
            places += &format!("  Own{i}:\n    capacity: single\n");
        }
        let outs: Vec<String> = t.outs.iter().map(|&o| place(i, o)).collect();
        let list = |v: &[String]| v.iter().map(|p| format!("    - {p}\n")).collect::<String>();
        transitions += &format!(
            "  T{i}:\n    trigger: {}\n    priority: {}\n    consumes:\n    - In{i}\n    reads:\n    - Go\n    inhibits:\n{}    produces:\n{}",
            if t.manual { "manual" } else { "auto" },
            t.priority,
            list(&outs),
            list(&outs)
        );
    }
    format!("name: priority\nplaces:\n{places}transitions:\n{transitions}")
}

fn reference(ts: &[T], subjects: usize) -> BTreeMap<String, Vec<String>> {
    let pref = |j: usize, i: usize| {
        (ts[j].priority, std::cmp::Reverse(j)) > (ts[i].priority, std::cmp::Reverse(i))
    };
    let outs = |i: usize| -> BTreeSet<String> { ts[i].outs.iter().map(|&o| place(i, o)).collect() };
    let mut all = BTreeMap::new();
    for s in 0..subjects {
        let mut filled = BTreeSet::new();
        let mut done = BTreeSet::new();
        let mut seq = Vec::new();
        loop {
            let cand: Vec<usize> = (0..ts.len())
                .filter(|&i| !ts[i].manual && ts[i].enabled[s] && !done.contains(&i))
                .filter(|&i| outs(i).is_disjoint(&filled))
                .collect();
            let fire: Vec<usize> = cand
                .iter()
                .copied()
                .filter(|&i| {
                    !cand
                        .iter()
                        .any(|&j| j != i && !outs(j).is_disjoint(&outs(i)) && pref(j, i))
                })
                .collect();
            if fire.is_empty() {
                break;
            }
            for &i in &fire {
                filled.extend(outs(i));
                done.insert(i);
                seq.push(format!("T{i}"));
            }
        }
        all.insert(format!("s{s}"), seq);
    }
    all
}

fn fired(ts: &[T], subjects: usize) -> BTreeMap<String, Vec<String>> {
    let order = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let o = order.clone();
    let outs: Vec<Vec<String>> = ts
        .iter()
        .enumerate()
        .map(|(i, t)| t.outs.iter().map(|&p| place(i, p)).collect())
        .collect();
    let logic = move |name: &str| {
        if name == "Fan" {
            return Logic::pure(
                Arc::new(|_: &mut Reader<'_>, _: &Subject| Some(Content::new())),
                Arc::new(move |_: &Subject, _: &Content| {
                    Ok((0..subjects)
                        .map(|s| Output::new("Go", &Subject::new(&format!("s{s}")), Content::new()))
                        .collect())
                }),
            );
        }
        let i: usize = name[1..].parse().unwrap();
        let (o, name, out) = (o.clone(), name.to_string(), outs[i].clone());
        Logic::pure(
            Arc::new(|_: &mut Reader<'_>, _: &Subject| Some(Content::new())),
            Arc::new(move |s: &Subject, _: &Content| {
                o.lock().unwrap().push((s.to_string(), name.clone()));
                let by = Content::from([("by".to_string(), Value::String(name.clone()))]);
                Ok(out.iter().map(|p| Output::new(p, s, by.clone())).collect())
            }),
        )
    };
    let net = net_def("nets/priority.yaml", &yaml(ts), logic).expect("loads");
    let mut exec = Executor::new(net).expect("accepted");
    for (i, t) in ts.iter().enumerate() {
        for s in 0..subjects {
            if t.enabled[s] {
                let _q = exec
                    .write(
                        &format!("In{i}"),
                        &format!("s{s}"),
                        Content::new(),
                        Duration::ZERO,
                    )
                    .expect("write");
            }
        }
    }
    let _q = exec
        .write("Start", "g", Content::new(), Duration::ZERO)
        .expect("start");
    let mut all: BTreeMap<String, Vec<String>> = (0..subjects)
        .map(|s| (format!("s{s}"), Vec::new()))
        .collect();
    for (s, n) in order.lock().unwrap().iter() {
        all.get_mut(s).expect("known subject").push(n.clone());
    }
    all
}

fn drawn() -> impl Strategy<Value = (Vec<T>, usize)> {
    (1..4usize).prop_flat_map(|subjects| {
        (
            prop::collection::vec(
                (
                    prop::bool::weighted(0.2),
                    -1..3i32,
                    prop::collection::btree_set(0..=SHARED, 1..3),
                    prop::collection::vec(any::<bool>(), subjects),
                )
                    .prop_map(|(manual, priority, outs, enabled)| T {
                        manual,
                        priority,
                        outs,
                        enabled,
                    }),
                1..8,
            ),
            Just(subjects),
        )
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, ..ProptestConfig::default() })]

    #[test]
    fn verify_priority_multi_place_multi_subject((ts, subjects) in drawn()) {
        prop_assert_eq!(fired(&ts, subjects), reference(&ts, subjects), "net:\n{}", yaml(&ts));
    }
}
