use std::collections::BTreeMap;
use std::collections::BTreeSet;

use crate::Applied;
use crate::At;
use crate::Discipline;
use crate::Fire;
use crate::Kinds;
use crate::Net;
use crate::Out;
use crate::Store;
use crate::TransitionId;
use crate::Trigger;
use crate::view::View;

/// Why a (transition, key) pair cannot fire. Arc indices count within the
/// transition's `consumes` or `inhibits`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WhyNot<P, Key> {
    /// An auto transition got a binding, or an external one got none.
    WrongTrigger,
    ConsumeEmpty {
        arc: usize,
    },
    Inhibited {
        arc: usize,
    },
    ReadsUndeclared(P),
    BindDeclined,
    ProducesUndeclared(P),
    /// A token into a Counted place, or a unit into a token place.
    OutputKind(P),
    /// A unit consumed from a Counted place has no token to move.
    MovedCounted {
        arc: usize,
    },
    OutputFull(P, Key),
    /// A move to the front of a row whose place is not Fifo.
    FrontNotFifo(P),
}

/// What an evaluation looked at, so a cache knows when to look again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Footprint<P: Ord, Key: Ord, R> {
    /// Arc rows and rows the bind read, present or absent, with their row
    /// stamps.
    pub rows: BTreeMap<(P, Key), R>,
    /// Places the bind listed the keys of.
    pub scans: BTreeSet<P>,
    /// Output rows whose discipline can refuse (Single, Counted).
    pub outputs: BTreeMap<(P, Key), R>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fired<Key, Stamp> {
    pub transition: TransitionId,
    pub key: Key,
    /// The stamps of the firing's new tokens, in output order.
    pub stamps: Vec<Stamp>,
}

/// `settle` hit its bound; the firings before it are applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unsettled<Key, Stamp> {
    pub fired: Vec<Fired<Key, Stamp>>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum StoreMismatch<P, Key> {
    Places {
        net: Vec<(P, Discipline)>,
        store: Vec<(P, Discipline)>,
    },
    /// A row holds more than its discipline allows.
    Row { place: P, key: Key, len: usize },
}

pub type FiredOf<K, S> = Fired<<K as Kinds>::Key, <S as Store<K>>::Stamp>;

/// What `settle` returns.
pub type Settled<K, S> =
    Result<Vec<FiredOf<K, S>>, Unsettled<<K as Kinds>::Key, <S as Store<K>>::Stamp>>;

pub type Evaluation<K, R> = (
    Result<Fire<K>, WhyNot<<K as Kinds>::Place, <K as Kinds>::Key>>,
    Footprint<<K as Kinds>::Place, <K as Kinds>::Key, R>,
);

impl<K: Kinds> Net<K> {
    /// Checks the store against the net (places, disciplines, row sizes).
    pub fn bind<'a, S: Store<K>>(
        &'a self,
        store: &'a mut S,
    ) -> Result<Bound<'a, K, S>, StoreMismatch<K::Place, K::Key>> {
        let net: Vec<_> = self.places().collect();
        let in_store = store.places();
        if net != in_store {
            return Err(StoreMismatch::Places {
                net,
                store: in_store,
            });
        }
        for (place, discipline) in net {
            for key in store.keys(&place) {
                let len = store.row(&place, &key).len();
                let max = match discipline {
                    Discipline::Single | Discipline::Register => 1,
                    Discipline::Fifo => usize::MAX,
                    Discipline::Counted { size } => size as usize,
                };
                if len > max {
                    return Err(StoreMismatch::Row { place, key, len });
                }
            }
        }
        Ok(Bound {
            net: self,
            store,
            settled: false,
        })
    }
}

/// A net bound to its store. External firings are only legal at quiescence:
/// `settled` is true exactly when the last call was a `settle` that ended
/// settled, so an external firing needs a settle before it.
pub struct Bound<'a, K: Kinds, S> {
    net: &'a Net<K>,
    store: &'a mut S,
    settled: bool,
}

impl<K: Kinds, S: Store<K>> Bound<'_, K, S> {
    pub fn store(&self) -> &S {
        self.store
    }

    pub fn enabled(
        &self,
        t: TransitionId,
        key: &K::Key,
        binding: Option<&K::Binding>,
    ) -> Evaluation<K, S::RowStamp> {
        let net = self.net;
        let store = &*self.store;
        let tr = net.transition(t);
        let mut fp = Footprint {
            rows: BTreeMap::new(),
            scans: BTreeSet::new(),
            outputs: BTreeMap::new(),
        };
        let stamped = |row: (K::Place, K::Key)| {
            let stamp = store.row_stamp(&row.0, &row.1);
            (row, stamp)
        };
        let consumes: Vec<_> = tr.consumes.iter().map(|a| a.row(key)).collect();
        let arc_rows = consumes
            .iter()
            .cloned()
            .chain(tr.inhibits.iter().map(|a| a.row(key)));
        fp.rows.extend(arc_rows.map(stamped));
        if (tr.trigger == Trigger::Auto) != binding.is_none() {
            return (Err(WhyNot::WrongTrigger), fp);
        }
        let mut taken: BTreeMap<&(K::Place, K::Key), usize> = BTreeMap::new();
        for (arc, row) in consumes.iter().enumerate() {
            let n = taken.entry(row).or_insert(0);
            *n += 1;
            if store.row(&row.0, &row.1).len() < *n {
                return (Err(WhyNot::ConsumeEmpty { arc }), fp);
            }
        }
        for (arc, a) in tr.inhibits.iter().enumerate() {
            let (p, k) = a.row(key);
            if !store.row(&p, &k).is_empty() {
                return (Err(WhyNot::Inhibited { arc }), fp);
            }
        }
        let mut view = View::new(store, &net.readable[t.0]);
        let fire = (tr.bind)(&mut view, key, binding);
        fp.rows.extend(view.rows.iter().cloned().map(stamped));
        fp.scans = view.scans;
        if let Some(p) = view.undeclared {
            return (Err(WhyNot::ReadsUndeclared(p)), fp);
        }
        let Some(fire) = fire else {
            return (Err(WhyNot::BindDeclined), fp);
        };
        let mut moved = vec![false; consumes.len()];
        let mut sizes: BTreeMap<(K::Place, K::Key), usize> = BTreeMap::new();
        for (p, k, out) in &fire.outputs {
            if !tr.produces.contains(p) {
                return (Err(WhyNot::ProducesUndeclared(p.clone())), fp);
            }
            let discipline = net.places[p];
            match out {
                Out::Unit if !discipline.is_counted() => {
                    return (Err(WhyNot::OutputKind(p.clone())), fp);
                }
                Out::New(_) | Out::Moved { .. } if discipline.is_counted() => {
                    return (Err(WhyNot::OutputKind(p.clone())), fp);
                }
                Out::Moved { at: At::Front, .. } if discipline != Discipline::Fifo => {
                    return (Err(WhyNot::FrontNotFifo(p.clone())), fp);
                }
                Out::Moved { from: i, .. } => {
                    assert!(
                        *i < consumes.len(),
                        "{}: Moved from {i} with {} consumes",
                        tr.name,
                        consumes.len()
                    );
                    if net.places[&consumes[*i].0].is_counted() {
                        return (Err(WhyNot::MovedCounted { arc: *i }), fp);
                    }
                    assert!(!moved[*i], "{}: consumed token {i} moved twice", tr.name);
                    moved[*i] = true;
                }
                _ => {}
            }
            let max = match discipline {
                Discipline::Single => 1,
                Discipline::Counted { size } => size as usize,
                Discipline::Register | Discipline::Fifo => continue,
            };
            let row = (p.clone(), k.clone());
            fp.outputs.insert(row.clone(), store.row_stamp(p, k));
            let len = sizes.entry(row.clone()).or_insert_with(|| {
                store.row(p, k).len() - consumes.iter().filter(|c| **c == row).count()
            });
            *len += 1;
            if *len > max {
                return (Err(WhyNot::OutputFull(p.clone(), k.clone())), fp);
            }
        }
        (Ok(fire), fp)
    }

    fn apply(&mut self, t: TransitionId, key: K::Key, fire: Fire<K>) -> FiredOf<K, S> {
        let consumes = self
            .net
            .transition(t)
            .consumes
            .iter()
            .map(|a| a.row(&key))
            .collect();
        let stamps = self.store.apply(Applied {
            consumes,
            outputs: fire.outputs,
        });
        Fired {
            transition: t,
            key,
            stamps,
        }
    }

    /// Only at quiescence (asserted); a refusal leaves the store unchanged.
    pub fn fire_external(
        &mut self,
        t: TransitionId,
        key: K::Key,
        binding: K::Binding,
    ) -> Result<FiredOf<K, S>, WhyNot<K::Place, K::Key>> {
        assert!(
            self.settled,
            "external firing of {:?} while the net is not settled",
            self.net.transition(t)
        );
        let fire = self.enabled(t, &key, Some(&binding)).0?;
        self.settled = false;
        Ok(self.apply(t, key, fire))
    }

    fn first_enabled(&self) -> Option<(TransitionId, K::Key, Fire<K>)> {
        for &t in &self.net.auto_order {
            let tr = self.net.transition(t);
            let own = tr
                .consumes
                .iter()
                .filter(|a| matches!(a.key, crate::KeySel::Own))
                .map(|a| &a.place);
            let keys: BTreeSet<K::Key> = own
                .chain(&tr.same_key_reads)
                .flat_map(|p| self.store.keys(p))
                .collect();
            for key in keys {
                if let Ok(fire) = self.enabled(t, &key, None).0 {
                    return Some((t, key, fire));
                }
            }
        }
        None
    }

    /// The pair `settle` fires next: highest priority, then declaration order,
    /// then key order.
    pub fn next_enabled(&self) -> Option<(TransitionId, K::Key)> {
        self.first_enabled().map(|(t, k, _)| (t, k))
    }

    /// Level semantics, interleaving: fire the first enabled auto pair, check
    /// again.
    pub fn settle(&mut self, bound: usize) -> Settled<K, S> {
        let mut fired = Vec::new();
        while let Some((t, key, fire)) = self.first_enabled() {
            if fired.len() == bound {
                self.settled = false;
                return Err(Unsettled { fired });
            }
            fired.push(self.apply(t, key, fire));
        }
        self.settled = true;
        Ok(fired)
    }
}
