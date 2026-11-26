//! The L0 scenarios of the concept as explicit nets. Example-based on purpose:
//! each pins one named scenario; the drawn-net laws cover the classes.

use std::sync::Arc;

use petri_net::ArcSpec;
use petri_net::BuildError;
use petri_net::Discipline;
use petri_net::Fire;
use petri_net::KeySel;
use petri_net::Kinds;
use petri_net::MemRowStamp;
use petri_net::MemStore;
use petri_net::Net;
use petri_net::NetBuilder;
use petri_net::Out;
use petri_net::RowView;
use petri_net::Store;
use petri_net::Transition;
use petri_net::Trigger;
use petri_net::View;
use petri_net::WhyNot;

#[derive(Debug)]
struct K;

impl Kinds for K {
    type Place = &'static str;
    type Key = &'static str;
    type Token = u32;
    type Binding = u32;
}

type Bind = fn(&mut View<'_, K>, &&'static str, Option<&u32>) -> Option<Fire<K>>;

fn own(place: &'static str) -> ArcSpec<K> {
    ArcSpec {
        place,
        key: KeySel::Own,
    }
}

fn t(name: &str, trigger: Trigger, bind: Bind) -> Transition<K> {
    Transition {
        name: name.into(),
        trigger,
        priority: 0,
        consumes: vec![],
        reads: vec![],
        same_key_reads: vec![],
        inhibits: vec![],
        produces: vec![],
        bind: Arc::new(bind),
    }
}

fn out(place: &'static str, key: &'static str, o: Out<K>) -> Fire<K> {
    Fire {
        outputs: vec![(place, key, o)],
    }
}

/// An external writer: the binding becomes a new token in `place[key]`.
fn writer(place: &'static str) -> Transition<K> {
    let mut w = t("write", Trigger::External, |_, _, _| unreachable!());
    w.produces = vec![place];
    w.bind = Arc::new(move |_, key, b| Some(out(place, key, Out::New(*b.unwrap()))));
    w
}

fn store(net: &Net<K>) -> MemStore<K> {
    MemStore::new(net.places(), 3)
}

fn tokens<S: Store<K>>(s: &S, place: &'static str, key: &'static str) -> Vec<u32> {
    match s.row(&place, &key) {
        RowView::Tokens(t) => t.into_iter().copied().collect(),
        RowView::Count(_) => panic!("{place} is Counted"),
    }
}

#[test]
fn two_producers_with_an_inhibitor_on_a_shared_empty_output() {
    let mut b = NetBuilder::default();
    b.place("A", Discipline::Fifo)
        .place("B", Discipline::Fifo)
        .place("Out", Discipline::Single);
    let mut seed = t("seed", Trigger::External, |_, k, _| {
        Some(Fire {
            outputs: vec![("A", k, Out::New(1)), ("B", k, Out::New(2))],
        })
    });
    seed.produces = vec!["A", "B"];
    let seed = b.transition(seed);
    let producer = |name, from: &'static str, bind: Bind| {
        let mut p = t(name, Trigger::Auto, bind);
        p.consumes = vec![own(from)];
        p.inhibits = vec![own("Out")];
        p.produces = vec!["Out"];
        p
    };
    let p1 = b.transition(producer("p1", "A", |_, k, _| {
        Some(out("Out", k, Out::Moved(0)))
    }));
    let p2 = b.transition(producer("p2", "B", |_, k, _| {
        Some(out("Out", k, Out::Moved(0)))
    }));
    let net = b.build().unwrap();
    let mut s = store(&net);
    let mut bound = net.bind(&mut s).unwrap();
    bound.settle(10).unwrap();
    bound.fire_external(seed, "s", 0).unwrap();
    let fired = bound.settle(10).unwrap();
    assert_eq!(
        fired.iter().map(|f| f.transition).collect::<Vec<_>>(),
        vec![p1],
        "declaration order breaks the tie"
    );
    assert_eq!(
        bound.enabled(p2, &"s", None).0.unwrap_err(),
        WhyNot::Inhibited { arc: 0 }
    );
    assert_eq!(
        (
            tokens(bound.store(), "Out", "s"),
            tokens(bound.store(), "B", "s")
        ),
        (vec![1], vec![2])
    );
}

#[test]
fn a_higher_priority_fires_before_an_earlier_declaration() {
    let mut b = NetBuilder::default();
    b.place("A", Discipline::Single)
        .place("Out", Discipline::Fifo);
    let w = b.transition(writer("A"));
    let mut low = t("low", Trigger::Auto, |_, k, _| {
        Some(out("Out", k, Out::New(10)))
    });
    let mut high = t("high", Trigger::Auto, |_, k, _| {
        Some(out("Out", k, Out::New(20)))
    });
    for (tr, priority) in [(&mut low, 0), (&mut high, 1)] {
        tr.priority = priority;
        tr.consumes = vec![own("A")];
        tr.produces = vec!["Out"];
    }
    b.transition(low);
    let high = b.transition(high);
    let net = b.build().unwrap();
    let mut s = store(&net);
    let mut bound = net.bind(&mut s).unwrap();
    bound.settle(10).unwrap();
    bound.fire_external(w, "s", 1).unwrap();
    assert_eq!(bound.settle(10).unwrap()[0].transition, high);
}

/// FetchFeed take and fail: the token moves into Running and back, and the
/// row stamp after the round trip equals the stamp before the take.
#[test]
fn fetch_feed_take_and_fail_keep_the_row_stamp() {
    let mut b = NetBuilder::default();
    b.place("Feed", Discipline::Single)
        .place("Running", Discipline::Single);
    let w = b.transition(writer("Feed"));
    let mut take = t("take", Trigger::Auto, |_, k, _| {
        Some(out("Running", k, Out::Moved(0)))
    });
    take.consumes = vec![own("Feed")];
    take.produces = vec!["Running"];
    let take = b.transition(take);
    let mut fail = t("fail", Trigger::External, |_, k, _| {
        Some(out("Feed", k, Out::Moved(0)))
    });
    fail.consumes = vec![own("Running")];
    fail.produces = vec!["Feed"];
    let fail = b.transition(fail);
    let net = b.build().unwrap();
    let mut s = store(&net);
    let mut bound = net.bind(&mut s).unwrap();
    bound.settle(10).unwrap();
    bound.fire_external(w, "s", 7).unwrap();
    let before = bound.store().row_stamp(&"Feed", &"s");
    let (fire, fp) = bound.enabled(take, &"s", None);
    fire.unwrap();
    assert_eq!(
        fp.rows[&("Feed", "s")],
        before,
        "the take's footprint holds the stamp it read"
    );
    assert_eq!(bound.settle(10).unwrap().len(), 1);
    assert_eq!(
        bound.store().row_stamp(&"Running", &"s"),
        before,
        "a move keeps the stamp"
    );
    assert_eq!(
        bound.store().row_stamp(&"Feed", &"s"),
        MemRowStamp::Tokens(vec![])
    );
    bound.fire_external(fail, "s", 0).unwrap();
    // `take` fires again at once: the level is high again.
    assert_eq!(bound.settle(10).unwrap().len(), 1);
    assert_eq!(bound.store().row_stamp(&"Running", &"s"), before);
    assert_eq!(tokens(bound.store(), "Running", "s"), vec![7]);
}

#[test]
fn a_pool_unit_is_taken_and_returned() {
    let mut b = NetBuilder::default();
    b.place("Pool", Discipline::Counted { size: 1 })
        .place("Feed", Discipline::Fifo)
        .place("Running", Discipline::Single);
    let mut fill = t("fill", Trigger::External, |_, _, _| {
        Some(out("Pool", "pool", Out::Unit))
    });
    fill.produces = vec!["Pool"];
    let fill = b.transition(fill);
    let mut seed = t("seed", Trigger::External, |_, _, _| {
        Some(Fire {
            outputs: vec![("Feed", "a", Out::New(1)), ("Feed", "b", Out::New(2))],
        })
    });
    seed.produces = vec!["Feed"];
    let seed = b.transition(seed);
    let mut take = t("take", Trigger::Auto, |_, k, _| {
        Some(out("Running", k, Out::Moved(0)))
    });
    take.consumes = vec![
        own("Feed"),
        ArcSpec {
            place: "Pool",
            key: KeySel::Fixed("pool"),
        },
    ];
    take.produces = vec!["Running"];
    let take = b.transition(take);
    let mut fail = t("fail", Trigger::External, |_, k, _| {
        Some(Fire {
            outputs: vec![("Feed", k, Out::Moved(0)), ("Pool", "pool", Out::Unit)],
        })
    });
    fail.consumes = vec![own("Running")];
    fail.produces = vec!["Feed", "Pool"];
    let fail = b.transition(fail);
    let net = b.build().unwrap();
    let mut s = store(&net);
    let mut bound = net.bind(&mut s).unwrap();
    bound.settle(10).unwrap();
    bound.fire_external(fill, "x", 0).unwrap();
    bound.settle(10).unwrap();
    assert_eq!(
        bound.fire_external(fill, "x", 0).unwrap_err(),
        WhyNot::OutputFull("Pool", "pool")
    );
    bound.fire_external(seed, "x", 0).unwrap();
    let fired = bound.settle(10).unwrap();
    assert_eq!(
        fired
            .iter()
            .map(|f| (f.transition, f.key))
            .collect::<Vec<_>>(),
        vec![(take, "a")]
    );
    assert!(matches!(
        bound.store().row(&"Pool", &"pool"),
        RowView::Count(0)
    ));
    assert_eq!(
        bound.enabled(take, &"b", None).0.unwrap_err(),
        WhyNot::ConsumeEmpty { arc: 1 }
    );
    bound.fire_external(fail, "a", 0).unwrap();
    assert!(
        matches!(bound.store().row(&"Pool", &"pool"), RowView::Count(1)),
        "fail returns the unit"
    );
    let fired = bound.settle(10).unwrap();
    assert_eq!(
        fired
            .iter()
            .map(|f| (f.transition, f.key))
            .collect::<Vec<_>>(),
        vec![(take, "a")],
        "key order"
    );
}

#[test]
fn fifo_consume_takes_the_head_and_the_next_keeps_its_stamp() {
    let mut b = NetBuilder::default();
    b.place("Q", Discipline::Fifo)
        .place("Done", Discipline::Fifo);
    let w = b.transition(writer("Q"));
    let mut take = t("take", Trigger::External, |_, k, _| {
        Some(out("Done", k, Out::Moved(0)))
    });
    take.consumes = vec![own("Q")];
    take.produces = vec!["Done"];
    let take = b.transition(take);
    let net = b.build().unwrap();
    let mut s = store(&net);
    let mut bound = net.bind(&mut s).unwrap();
    for v in [1, 2, 3] {
        bound.settle(10).unwrap();
        bound.fire_external(w, "s", v).unwrap();
    }
    bound.settle(10).unwrap();
    let MemRowStamp::Tokens(stamps) = bound.store().row_stamp(&"Q", &"s") else {
        unreachable!()
    };
    bound.fire_external(take, "s", 0).unwrap();
    assert_eq!(tokens(bound.store(), "Q", "s"), vec![2, 3]);
    assert_eq!(
        bound.store().row_stamp(&"Q", &"s"),
        MemRowStamp::Tokens(stamps[1..].to_vec())
    );
    assert_eq!(
        bound.store().row_stamp(&"Done", &"s"),
        MemRowStamp::Tokens(stamps[..1].to_vec())
    );
}

#[test]
fn register_replaces_and_single_refuses_a_full_row() {
    let mut b = NetBuilder::default();
    b.place("R", Discipline::Register)
        .place("S", Discipline::Single);
    let wr = b.transition(writer("R"));
    let ws = b.transition(writer("S"));
    let net = b.build().unwrap();
    let mut s = store(&net);
    let mut bound = net.bind(&mut s).unwrap();
    for (w, v) in [(wr, 1), (wr, 2), (ws, 1)] {
        bound.settle(10).unwrap();
        bound.fire_external(w, "s", v).unwrap();
    }
    bound.settle(10).unwrap();
    assert_eq!(tokens(bound.store(), "R", "s"), vec![2]);
    let before = bound.store().row_stamp(&"S", &"s");
    let (refused, fp) = bound.enabled(ws, &"s", Some(&2));
    assert_eq!(refused.unwrap_err(), WhyNot::OutputFull("S", "s"));
    assert_eq!(
        fp.outputs[&("S", "s")],
        before,
        "the refusing output row is in the footprint"
    );
    assert_eq!(
        bound.fire_external(ws, "s", 2).unwrap_err(),
        WhyNot::OutputFull("S", "s")
    );
    assert_eq!(
        (
            tokens(bound.store(), "S", "s"),
            bound.store().row_stamp(&"S", &"s")
        ),
        (vec![1], before)
    );
}

#[test]
fn an_auto_transition_that_never_disables_itself_is_unsettled_with_its_prefix() {
    let mut b = NetBuilder::default();
    b.place("P", Discipline::Fifo);
    let w = b.transition(writer("P"));
    let mut spin = t("spin", Trigger::Auto, |_, k, _| {
        Some(out("P", k, Out::New(0)))
    });
    spin.same_key_reads = vec!["P"];
    spin.produces = vec!["P"];
    b.transition(spin);
    let net = b.build().unwrap();
    let mut s = store(&net);
    let mut bound = net.bind(&mut s).unwrap();
    bound.settle(10).unwrap();
    bound.fire_external(w, "s", 1).unwrap();
    let unsettled = bound.settle(5).unwrap_err();
    assert_eq!(unsettled.fired.len(), 5);
    assert_eq!(
        tokens(bound.store(), "P", "s"),
        vec![1, 0, 0, 0, 0, 0],
        "the prefix is applied"
    );
}

#[test]
fn a_read_of_an_undeclared_place_is_refused() {
    let mut b = NetBuilder::default();
    b.place("A", Discipline::Single)
        .place("Secret", Discipline::Single);
    let mut peek = t("peek", Trigger::External, |v, k, _| {
        v.row(&"Secret", k)?;
        Some(Fire { outputs: vec![] })
    });
    peek.reads = vec!["A"];
    let peek = b.transition(peek);
    let net = b.build().unwrap();
    let mut s = store(&net);
    let bound = net.bind(&mut s).unwrap();
    assert_eq!(
        bound.enabled(peek, &"s", Some(&0)).0.unwrap_err(),
        WhyNot::ReadsUndeclared("Secret")
    );
}

#[test]
fn moves_into_or_out_of_a_counted_place_are_refused() {
    let mut b = NetBuilder::default();
    b.place("Pool", Discipline::Counted { size: 2 })
        .place("A", Discipline::Fifo);
    let mut fill = t("fill", Trigger::External, |_, k, _| {
        Some(out("Pool", k, Out::Unit))
    });
    fill.produces = vec!["Pool"];
    let fill = b.transition(fill);
    let wa = b.transition(writer("A"));
    let mut out_of = t("out_of", Trigger::External, |_, k, _| {
        Some(out("A", k, Out::Moved(0)))
    });
    out_of.consumes = vec![own("Pool")];
    out_of.produces = vec!["A"];
    let out_of = b.transition(out_of);
    let mut into = t("into", Trigger::External, |_, k, _| {
        Some(out("Pool", k, Out::Moved(0)))
    });
    into.consumes = vec![own("A")];
    into.produces = vec!["Pool"];
    let into = b.transition(into);
    let net = b.build().unwrap();
    let mut s = store(&net);
    let mut bound = net.bind(&mut s).unwrap();
    for w in [fill, wa] {
        bound.settle(10).unwrap();
        bound.fire_external(w, "s", 1).unwrap();
    }
    bound.settle(10).unwrap();
    assert_eq!(
        bound.fire_external(out_of, "s", 0).unwrap_err(),
        WhyNot::MovedCounted { arc: 0 }
    );
    assert_eq!(
        bound.fire_external(into, "s", 0).unwrap_err(),
        WhyNot::OutputKind("Pool")
    );
}

#[test]
#[should_panic(expected = "moved twice")]
fn a_consumed_token_moved_twice_is_an_assert() {
    let mut b = NetBuilder::default();
    b.place("A", Discipline::Fifo);
    let w = b.transition(writer("A"));
    let mut dup = t("dup", Trigger::External, |_, k, _| {
        Some(Fire {
            outputs: vec![("A", k, Out::Moved(0)), ("A", k, Out::Moved(0))],
        })
    });
    dup.consumes = vec![own("A")];
    dup.produces = vec!["A"];
    let dup = b.transition(dup);
    let net = b.build().unwrap();
    let mut s = store(&net);
    let mut bound = net.bind(&mut s).unwrap();
    bound.settle(10).unwrap();
    bound.fire_external(w, "s", 1).unwrap();
    bound.settle(10).unwrap();
    let _ = bound.fire_external(dup, "s", 0);
}

#[test]
#[should_panic(expected = "not settled")]
fn an_external_firing_before_a_settle_is_an_assert() {
    let mut b = NetBuilder::default();
    b.place("A", Discipline::Fifo);
    let w = b.transition(writer("A"));
    let net = b.build().unwrap();
    let mut s = store(&net);
    let mut bound = net.bind(&mut s).unwrap();
    bound.settle(10).unwrap();
    bound.fire_external(w, "s", 1).unwrap();
    let _ = bound.fire_external(w, "s", 2);
}

#[test]
fn build_refuses_undeclared_places_duplicates_and_keyless_auto_transitions() {
    let build = |places: &[&'static str], tr: Transition<K>| {
        let mut b = NetBuilder::default();
        for p in places {
            b.place(*p, Discipline::Fifo);
        }
        b.transition(tr);
        b.build().err()
    };
    let mut reads_b = t("r", Trigger::External, |_, _, _| None);
    reads_b.reads = vec!["B"];
    assert_eq!(
        build(&["A"], reads_b),
        Some(BuildError::UndeclaredPlace {
            transition: "r".into(),
            place: "B"
        })
    );
    let mut fixed_only = t("auto", Trigger::Auto, |_, _, _| None);
    fixed_only.consumes = vec![ArcSpec {
        place: "A",
        key: KeySel::Fixed("x"),
    }];
    fixed_only.reads = vec!["A"];
    assert_eq!(
        build(&["A"], fixed_only),
        Some(BuildError::AutoWithoutKeyArc {
            transition: "auto".into()
        })
    );
    assert_eq!(
        build(&["A", "A"], t("e", Trigger::External, |_, _, _| None)),
        Some(BuildError::DuplicatePlace("A"))
    );
}

#[test]
fn a_store_whose_places_differ_from_the_net_is_refused_at_bind() {
    let mut b = NetBuilder::default();
    b.place("A", Discipline::Single);
    let net = b.build().unwrap();
    let mut s: MemStore<K> = MemStore::new([("A", Discipline::Fifo)], 0);
    assert!(matches!(
        net.bind(&mut s),
        Err(petri_net::StoreMismatch::Places { .. })
    ));
}
