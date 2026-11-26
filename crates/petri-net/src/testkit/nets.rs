//! The drawn-net generator: places of every discipline, transitions with
//! drawn arcs and key selectors, binds as data that read and produce, and
//! drawn external firings. Faults the net must refuse (undeclared reads and
//! produces, wrong output kinds, full output rows) are drawn on purpose.

use std::collections::BTreeSet;
use std::sync::Arc;

use proptest::prelude::*;

use super::{KEYS, TestKinds, arb_places};
use crate::{ArcSpec, Discipline, Fire, KeySel, Net, NetBuilder, Out, RowView, Transition, TransitionId, Trigger};

/// A place and, when fixed, the key of the row an arc touches.
pub type ArcS = (u8, Option<u8>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Guard {
    Always,
    /// Enabled when the sum of what the bind read is even.
    EvenSum,
}

#[derive(Clone, Debug)]
pub enum OutS {
    /// The token is the sum of what the bind read plus this offset.
    New(u32),
    Moved(usize),
    Unit,
}

#[derive(Clone, Debug)]
pub struct BindSpec {
    pub reads: Vec<ArcS>,
    pub scan: Option<u8>,
    pub guard: Guard,
    pub outputs: Vec<(ArcS, OutS)>,
}

#[derive(Clone, Debug)]
pub struct TSpec {
    pub trigger: Trigger,
    pub priority: i32,
    pub consumes: Vec<ArcS>,
    pub reads: Vec<u8>,
    pub same_key_reads: Vec<u8>,
    pub inhibits: Vec<ArcS>,
    pub produces: Vec<u8>,
    pub bind: BindSpec,
}

/// One writer per place (transition id = place index) comes first: it fires
/// externally with the binding as the new token (a unit for Counted).
#[derive(Clone, Debug)]
pub struct NetSpec {
    pub places: Vec<(u8, Discipline)>,
    pub transitions: Vec<TSpec>,
}

/// An external firing: an index into `NetSpec::externals` (modulo), a key, a binding.
pub type Call = (usize, u8, u32);

#[derive(Clone, Debug)]
pub struct NetCase {
    pub net: NetSpec,
    pub calls: Vec<Call>,
}

pub const SETTLE_BOUND: usize = 24;

fn arb_arc(n: u8) -> impl Strategy<Value = ArcS> {
    (0..n, prop::option::weighted(0.3, 0..KEYS))
}

fn arb_tspec(places: Vec<(u8, Discipline)>) -> impl Strategy<Value = TSpec> {
    let n = places.len() as u8;
    let arcs = |max| prop::collection::vec(arb_arc(n), 0..=max);
    let place_set = |max| prop::collection::vec(0..n, 0..=max);
    let out_kind = prop_oneof![(0u32..4).prop_map(OutS::New), (0usize..3).prop_map(OutS::Moved), Just(OutS::Unit)];
    let outputs = prop::collection::vec((arb_arc(n), out_kind, prop::bool::weighted(0.85)), 0..=3);
    let bind = (arcs(2), prop::option::weighted(0.2, 0..n), prop_oneof![Just(Guard::Always), Just(Guard::EvenSum)], outputs);
    let shape = (prop::bool::weighted(0.6), 0i32..2, arcs(2), place_set(1), place_set(1), arcs(1));
    let faults = (prop::bool::weighted(0.1), prop::bool::weighted(0.1));
    (shape, bind, faults).prop_map(move |(shape, bind, (undeclared_read, undeclared_produce))| {
        let (auto, priority, consumes, extra_reads, mut same_key_reads, inhibits) = shape;
        let (bind_reads, scan, guard, outputs) = bind;
        let mut moved = BTreeSet::new();
        let outputs: Vec<(ArcS, OutS)> = outputs
            .into_iter()
            .map(|(arc, kind, right)| {
                let counted = places[arc.0 as usize].1.is_counted();
                let kind = match kind {
                    _ if right && counted => OutS::Unit,
                    OutS::Unit if right => OutS::New(0),
                    OutS::Moved(i) if i < consumes.len() && moved.insert(i) => OutS::Moved(i),
                    OutS::Moved(_) => OutS::New(1),
                    kind => kind,
                };
                (arc, kind)
            })
            .collect();
        let mut reads: Vec<u8> = bind_reads.iter().map(|a| a.0).chain(scan).chain(extra_reads).collect();
        if undeclared_read && !reads.is_empty() {
            let dropped = reads[0];
            reads.retain(|p| *p != dropped);
            same_key_reads.retain(|p| *p != dropped);
        }
        let own_consume = consumes.iter().any(|(_, k)| k.is_none());
        if auto && !own_consume && same_key_reads.is_empty() {
            same_key_reads.push(0);
        }
        let mut produces: Vec<u8> = outputs.iter().map(|(a, _)| a.0).collect();
        if undeclared_produce && !produces.is_empty() {
            produces.remove(0);
        }
        TSpec {
            trigger: if auto { Trigger::Auto } else { Trigger::External },
            priority,
            consumes,
            reads,
            same_key_reads,
            inhibits,
            produces,
            bind: BindSpec { reads: bind_reads, scan, guard, outputs },
        }
    })
}

pub fn arb_net_spec() -> impl Strategy<Value = NetSpec> {
    arb_places(4).prop_flat_map(|places| {
        let ts = prop::collection::vec(arb_tspec(places.clone()), 1..=5);
        (Just(places), ts).prop_map(|(places, transitions)| NetSpec { places, transitions })
    })
}

pub fn arb_net_case() -> impl Strategy<Value = NetCase> {
    let calls = prop::collection::vec((0usize..16, 0..KEYS, 0u32..6), 0..20);
    (arb_net_spec(), calls).prop_map(|(net, calls)| NetCase { net, calls })
}

fn arc(a: &ArcS) -> ArcSpec<TestKinds> {
    ArcSpec { place: a.0, key: a.1.map_or(KeySel::Own, KeySel::Fixed) }
}

fn sum_of(row: Option<RowView<'_, TestKinds>>) -> u64 {
    match row {
        Some(RowView::Tokens(t)) => t.iter().map(|x| u64::from(**x)).sum(),
        Some(RowView::Count(n)) => u64::from(n),
        None => 0,
    }
}

impl NetSpec {
    pub fn build(&self) -> Net<TestKinds> {
        let mut b = NetBuilder::default();
        for &(p, d) in &self.places {
            b.place(p, d);
        }
        for &(p, d) in &self.places {
            b.transition(Transition {
                name: format!("write{p}"),
                trigger: Trigger::External,
                priority: 0,
                consumes: vec![],
                reads: vec![],
                same_key_reads: vec![],
                inhibits: vec![],
                produces: vec![p],
                bind: Arc::new(move |_, key, binding| {
                    let out = if d.is_counted() { Out::Unit } else { Out::New(*binding.unwrap()) };
                    Some(Fire { outputs: vec![(p, *key, out)] })
                }),
            });
        }
        for (i, t) in self.transitions.iter().enumerate() {
            let spec = t.clone();
            b.transition(Transition {
                name: format!("t{i}"),
                trigger: t.trigger,
                priority: t.priority,
                consumes: t.consumes.iter().map(arc).collect(),
                reads: t.reads.clone(),
                same_key_reads: t.same_key_reads.clone(),
                inhibits: t.inhibits.iter().map(arc).collect(),
                produces: t.produces.clone(),
                bind: Arc::new(move |view, key, binding| {
                    let row = |a: &ArcS| arc(a).row(key);
                    let mut sum = u64::from(binding.copied().unwrap_or(0));
                    for (p, k) in spec.consumes.iter().chain(&spec.bind.reads).map(row) {
                        sum += sum_of(view.row(&p, &k));
                    }
                    if let Some(p) = spec.bind.scan {
                        sum += view.keys(&p).map_or(0, |ks| ks.len() as u64);
                    }
                    if spec.bind.guard == Guard::EvenSum && sum % 2 == 1 {
                        return None;
                    }
                    let outputs = spec.bind.outputs.iter().map(|(a, o)| {
                        let (p, k) = row(a);
                        let out = match o {
                            OutS::New(off) => Out::New(sum as u32 + off),
                            OutS::Moved(i) => Out::Moved(*i),
                            OutS::Unit => Out::Unit,
                        };
                        (p, k, out)
                    });
                    Some(Fire { outputs: outputs.collect() })
                }),
            });
        }
        b.build().expect("the generator draws only nets that build")
    }

    pub fn externals(&self) -> Vec<TransitionId> {
        let n = self.places.len();
        let generated = self.transitions.iter().enumerate().filter(|(_, t)| t.trigger == Trigger::External);
        (0..n).map(TransitionId).chain(generated.map(|(i, _)| TransitionId(n + i))).collect()
    }
}
