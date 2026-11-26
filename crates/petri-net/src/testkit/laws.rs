//! Laws of a net bound to a store, over drawn `NetCase`s. Each takes a store
//! factory so stores of other crates can run them too.

use std::cell::RefCell;
use std::collections::BTreeMap;

use proptest::prelude::*;
use proptest::test_runner::TestCaseError;

use super::{Content, KEYS, NetCase, SETTLE_BOUND, TestKinds, content};
use crate::{Applied, Bound, Discipline, Net, Out, RowView, Store, TransitionId, Trigger, Unsettled, WhyNot};

type Row = (u8, u8);
type Places = BTreeMap<u8, Discipline>;

/// A store that logs every `row` and `keys` call.
pub struct Recording<S> {
    pub inner: S,
    pub log: RefCell<Vec<Access>>,
}

#[derive(Clone, Debug)]
pub enum Access {
    Row(u8, u8),
    Keys(u8),
}

impl<S: Store<TestKinds>> Store<TestKinds> for Recording<S> {
    type Stamp = S::Stamp;
    type RowStamp = S::RowStamp;

    fn row(&self, place: &u8, key: &u8) -> RowView<'_, TestKinds> {
        self.log.borrow_mut().push(Access::Row(*place, *key));
        self.inner.row(place, key)
    }
    fn row_stamp(&self, place: &u8, key: &u8) -> S::RowStamp {
        self.inner.row_stamp(place, key)
    }
    fn keys(&self, place: &u8) -> Vec<u8> {
        self.log.borrow_mut().push(Access::Keys(*place));
        self.inner.keys(place)
    }
    fn places(&self) -> Vec<(u8, Discipline)> {
        self.inner.places()
    }
    fn apply(&mut self, firing: Applied<TestKinds>) -> Vec<S::Stamp> {
        self.inner.apply(firing)
    }
}

fn contents<S: Store<TestKinds>>(store: &S) -> BTreeMap<Row, Content> {
    let rows = store.places().into_iter().flat_map(|(p, _)| (0..KEYS).map(move |k| (p, k)));
    rows.map(|r| (r, content(store, r))).collect()
}

fn marking<S: Store<TestKinds>>(store: &S) -> BTreeMap<Row, (Content, S::RowStamp)> {
    contents(store).into_iter().map(|(r, c)| (r, (c, store.row_stamp(&r.0, &r.1)))).collect()
}

/// The marking a firing must leave, computed on contents alone.
fn predict(places: &Places, before: BTreeMap<Row, Content>, consumes: &[Row], outputs: &[(u8, u8, Out<TestKinds>)]) -> BTreeMap<Row, Content> {
    let mut m = before;
    let mut taken = Vec::new();
    for row in consumes {
        match m.get_mut(row).unwrap() {
            Content::Tokens(v) => taken.push(Some(v.remove(0))),
            Content::Count(n) => {
                *n -= 1;
                taken.push(None);
            }
        }
    }
    for (p, k, out) in outputs {
        let token = match out {
            Out::Unit => {
                let Content::Count(n) = m.get_mut(&(*p, *k)).unwrap() else { unreachable!() };
                *n += 1;
                continue;
            }
            Out::New(t) => *t,
            Out::Moved(i) => taken[*i].unwrap(),
        };
        let Content::Tokens(v) = m.get_mut(&(*p, *k)).unwrap() else { unreachable!() };
        if places[p] == Discipline::Register {
            v.clear();
        }
        v.push(token);
    }
    m
}

fn consumes(net: &Net<TestKinds>, t: TransitionId, key: u8) -> Vec<Row> {
    net.transition(t).consumes.iter().map(|a| a.row(&key)).collect()
}

/// Settles one firing at a time, checking each against `predict`. False when
/// the bound is hit.
fn settle_stepwise<S: Store<TestKinds>>(net: &Net<TestKinds>, places: &Places, bound: &mut Bound<'_, TestKinds, S>) -> Result<bool, TestCaseError> {
    for _ in 0..SETTLE_BOUND {
        let Some((t, k)) = bound.next_enabled() else {
            prop_assert_eq!(bound.settle(0).unwrap(), vec![]);
            return Ok(true);
        };
        let fire = bound.enabled(t, &k, None).0.unwrap();
        let expected = predict(places, contents(bound.store()), &consumes(net, t, k), &fire.outputs);
        let (Ok(fired) | Err(Unsettled { fired })) = bound.settle(1);
        prop_assert_eq!((fired.len(), fired[0].transition, fired[0].key), (1, t, k));
        prop_assert_eq!(contents(bound.store()), expected, "auto firing {:?} for key {}", t, k);
    }
    Ok(false)
}

/// A firing applies all its outputs (as `predict` says) or, refused, changes nothing.
pub fn check_atomicity<S: Store<TestKinds>>(factory: impl Fn(&[(u8, Discipline)]) -> S, case: &NetCase) -> Result<(), TestCaseError> {
    let net = case.net.build();
    let places: Places = net.places().collect();
    let mut store = factory(&net.places().collect::<Vec<_>>());
    let mut bound = net.bind(&mut store).unwrap();
    let externals = case.net.externals();
    if !settle_stepwise(&net, &places, &mut bound)? {
        return Ok(());
    }
    for &(i, k, b) in &case.calls {
        let t = externals[i % externals.len()];
        let before = marking(bound.store());
        let expected = bound.enabled(t, &k, Some(&b)).0.ok().map(|fire| {
            let before = before.iter().map(|(r, (c, _))| (*r, c.clone())).collect();
            predict(&places, before, &consumes(&net, t, k), &fire.outputs)
        });
        match bound.fire_external(t, k, b) {
            Ok(_) => prop_assert_eq!(Some(contents(bound.store())), expected),
            Err(why) => {
                prop_assert!(expected.is_none());
                prop_assert_eq!(marking(bound.store()), before, "refused firing ({:?}) changed the store", why);
            }
        }
        if !settle_stepwise(&net, &places, &mut bound)? {
            return Ok(());
        }
    }
    Ok(())
}

fn probe<S: Store<TestKinds>>(net: &Net<TestKinds>, bound: &Bound<'_, TestKinds, Recording<S>>, binding: u32) -> Result<(), TestCaseError> {
    for t in net.transition_ids() {
        let b = (net.transition(t).trigger == Trigger::External).then_some(binding);
        for k in 0..KEYS {
            bound.store().log.borrow_mut().clear();
            let (eval, fp) = bound.enabled(t, &k, b.as_ref());
            for access in bound.store().log.borrow().iter() {
                match *access {
                    Access::Row(p, k) => prop_assert!(
                        fp.rows.contains_key(&(p, k)) || fp.outputs.contains_key(&(p, k)),
                        "{:?} key {}: row ({}, {}) read but not in the footprint ({:?})",
                        t,
                        k,
                        p,
                        k,
                        eval
                    ),
                    Access::Keys(p) => prop_assert!(fp.scans.contains(&p), "{:?}: scan of {} not in the footprint", t, p),
                }
            }
            let outputs = match &eval {
                Ok(fire) => fire.outputs.iter().map(|(p, k, _)| (*p, *k)).collect(),
                Err(WhyNot::OutputFull(p, k)) => vec![(*p, *k)],
                Err(_) => vec![],
            };
            for (p, k) in outputs {
                if matches!(net.places().nth(p as usize).unwrap().1, Discipline::Single | Discipline::Counted { .. }) {
                    prop_assert!(fp.outputs.contains_key(&(p, k)), "{:?}: output row ({}, {}) can refuse but is not in the footprint", t, p, k);
                }
            }
        }
    }
    Ok(())
}

/// Every row and scan an evaluation touched is in its footprint, refusals
/// included, and so is every output row whose discipline can refuse.
pub fn check_footprint<S: Store<TestKinds>>(factory: impl Fn(&[(u8, Discipline)]) -> S, case: &NetCase) -> Result<(), TestCaseError> {
    let net = case.net.build();
    let mut store = Recording { inner: factory(&net.places().collect::<Vec<_>>()), log: RefCell::new(vec![]) };
    let mut bound = net.bind(&mut store).unwrap();
    let externals = case.net.externals();
    if bound.settle(SETTLE_BOUND).is_err() {
        return Ok(());
    }
    probe(&net, &bound, 0)?;
    for &(i, k, b) in &case.calls {
        let _ = bound.fire_external(externals[i % externals.len()], k, b);
        if bound.settle(SETTLE_BOUND).is_err() {
            return Ok(());
        }
        probe(&net, &bound, b)?;
    }
    Ok(())
}
