//! ADR 0032 §9.1: priority acts only among the producers of one place.
//!
//! Each drawn net has auto transitions in file order. A transition either
//! shares an output place with other producers or has an output place of its
//! own. Every one consumes its own input, reads `Go` and is inhibited by its
//! output, so one write of `Go` enables the drawn set in one round. The
//! reference: per shared place the enabled producer with the highest priority
//! fires, the earlier in the file on equal priority; every other enabled
//! transition fires too; all fire in file order.

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
struct Drawn {
    /// `Some(g)` produces the shared place `Out{g}`, `None` a place of its own.
    shares: Option<usize>,
    priority: i32,
    enabled: bool,
}

fn output(i: usize, t: &Drawn) -> String {
    match t.shares {
        Some(g) => format!("Out{g}"),
        None => format!("Own{i}"),
    }
}

fn yaml(ts: &[Drawn]) -> String {
    let mut places = String::from("  Go:\n    capacity: single\n");
    for g in 0..SHARED {
        places += &format!("  Out{g}:\n    capacity: single\n");
    }
    let mut transitions = String::new();
    for (i, t) in ts.iter().enumerate() {
        places += &format!("  In{i}:\n    capacity: single\n");
        if t.shares.is_none() {
            places += &format!("  Own{i}:\n    capacity: single\n");
        }
        let out = output(i, t);
        transitions += &format!(
            "  T{i}:\n    trigger: auto\n    priority: {}\n    consumes:\n    - In{i}\n    reads:\n    - Go\n    inhibits:\n    - {out}\n    produces:\n    - {out}\n",
            t.priority
        );
    }
    format!("name: priority\nplaces:\n{places}transitions:\n{transitions}")
}

fn reference(ts: &[Drawn]) -> Vec<String> {
    let preferred = |j: usize, i: usize| {
        (ts[j].priority, std::cmp::Reverse(j)) > (ts[i].priority, std::cmp::Reverse(i))
    };
    (0..ts.len())
        .filter(|&i| ts[i].enabled)
        .filter(|&i| {
            !(0..ts.len()).any(|j| {
                ts[j].enabled
                    && ts[j].shares.is_some()
                    && ts[j].shares == ts[i].shares
                    && preferred(j, i)
            })
        })
        .map(|i| format!("T{i}"))
        .collect()
}

fn fired(ts: &[Drawn]) -> Vec<String> {
    let order = Arc::new(Mutex::new(Vec::<String>::new()));
    let outputs: Vec<String> = ts.iter().enumerate().map(|(i, t)| output(i, t)).collect();
    let o = order.clone();
    let logic = move |name: &str| {
        let i: usize = name[1..].parse().expect("transitions are named T<i>");
        let (o, name, out) = (o.clone(), name.to_string(), outputs[i].clone());
        Logic::pure(
            Arc::new(|_: &mut Reader<'_>, _: &Subject| Some(Content::new())),
            Arc::new(move |s: &Subject, _: &Content| {
                o.lock().unwrap().push(name.clone());
                let by = Content::from([("by".to_string(), Value::String(name.clone()))]);
                Ok(vec![Output::new(&out, s, by)])
            }),
        )
    };
    let net = net_def("nets/priority.yaml", &yaml(ts), logic).expect("the drawn net loads");
    let mut exec = Executor::new(net).expect("the executor accepts the drawn net");
    for (i, t) in ts.iter().enumerate() {
        if t.enabled {
            let _q = exec
                .write(&format!("In{i}"), "s", Content::new(), Duration::ZERO)
                .expect("an input write is accepted");
        }
    }
    assert!(order.lock().unwrap().is_empty(), "nothing fires before Go");
    let _q = exec
        .write("Go", "s", Content::new(), Duration::ZERO)
        .expect("the Go write is accepted");
    order.lock().unwrap().clone()
}

fn drawn() -> impl Strategy<Value = Vec<Drawn>> {
    prop::collection::vec(
        (
            prop_oneof![Just(None), (0..SHARED).prop_map(Some)],
            -1..3i32,
            any::<bool>(),
        )
            .prop_map(|(shares, priority, enabled)| Drawn {
                shares,
                priority,
                enabled,
            }),
        1..9,
    )
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn priority_orders_only_the_producers_of_one_place(ts in drawn()) {
        prop_assert_eq!(fired(&ts), reference(&ts), "net:\n{}", yaml(&ts));
    }
}
