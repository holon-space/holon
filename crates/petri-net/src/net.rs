use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use crate::view::View;
use crate::{Discipline, Kinds, Out};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// Fired by `settle` whenever enabled; takes no binding.
    Auto,
    /// Fired by a caller with a binding, only at quiescence.
    External,
}

/// Which row of the arc's place a firing for key `k` touches.
#[derive(Clone, Debug)]
pub enum KeySel<K: Kinds> {
    Own,
    Fixed(K::Key),
}

#[derive(Clone, Debug)]
pub struct ArcSpec<K: Kinds> {
    pub place: K::Place,
    pub key: KeySel<K>,
}

impl<K: Kinds> ArcSpec<K> {
    pub fn row(&self, own: &K::Key) -> (K::Place, K::Key) {
        let key = match &self.key {
            KeySel::Own => own.clone(),
            KeySel::Fixed(k) => k.clone(),
        };
        (self.place.clone(), key)
    }
}

/// Guard, binding and fire in one: `None` means not enabled. The binding is
/// `Some` exactly for external transitions.
pub type Bind<K> = Arc<
    dyn Fn(&mut View<'_, K>, &<K as Kinds>::Key, Option<&<K as Kinds>::Binding>) -> Option<Fire<K>> + Send + Sync,
>;

pub struct Transition<K: Kinds> {
    pub name: String,
    pub trigger: Trigger,
    /// Higher fires first.
    pub priority: i32,
    /// Each takes the head token of its row (Counted: one unit). The bind may read them.
    pub consumes: Vec<ArcSpec<K>>,
    /// Any key, through the view.
    pub reads: Vec<K::Place>,
    /// Like `reads`, and their keys are candidate keys of an auto transition.
    pub same_key_reads: Vec<K::Place>,
    /// The row must be empty (Counted: count 0).
    pub inhibits: Vec<ArcSpec<K>>,
    pub produces: Vec<K::Place>,
    pub bind: Bind<K>,
}

impl<K: Kinds> fmt::Debug for Transition<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Transition").field("name", &self.name).field("trigger", &self.trigger).finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct Fire<K: Kinds> {
    pub outputs: Vec<(K::Place, K::Key, Out<K>)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TransitionId(pub usize);

#[derive(Debug, PartialEq, Eq)]
pub enum BuildError<P> {
    DuplicatePlace(P),
    UndeclaredPlace { transition: String, place: P },
    /// An auto transition needs an own-key consume or a same-key read to draw its keys from.
    AutoWithoutKeyArc { transition: String },
}

pub struct NetBuilder<K: Kinds> {
    places: Vec<(K::Place, Discipline)>,
    transitions: Vec<Transition<K>>,
}

impl<K: Kinds> Default for NetBuilder<K> {
    fn default() -> Self {
        NetBuilder { places: vec![], transitions: vec![] }
    }
}

impl<K: Kinds> NetBuilder<K> {
    pub fn place(&mut self, place: K::Place, discipline: Discipline) -> &mut Self {
        self.places.push((place, discipline));
        self
    }

    pub fn transition(&mut self, t: Transition<K>) -> TransitionId {
        self.transitions.push(t);
        TransitionId(self.transitions.len() - 1)
    }

    pub fn build(self) -> Result<Net<K>, BuildError<K::Place>> {
        let mut places = BTreeMap::new();
        for (p, d) in self.places {
            if places.insert(p.clone(), d).is_some() {
                return Err(BuildError::DuplicatePlace(p));
            }
        }
        let mut readable = Vec::new();
        for t in &self.transitions {
            let arcs = t.consumes.iter().chain(&t.inhibits).map(|a| &a.place);
            let all: Vec<&K::Place> = arcs.chain(&t.reads).chain(&t.same_key_reads).chain(&t.produces).collect();
            if let Some(p) = all.into_iter().find(|p| !places.contains_key(*p)) {
                return Err(BuildError::UndeclaredPlace { transition: t.name.clone(), place: p.clone() });
            }
            let own_consume = t.consumes.iter().any(|a| matches!(a.key, KeySel::Own));
            if t.trigger == Trigger::Auto && !own_consume && t.same_key_reads.is_empty() {
                return Err(BuildError::AutoWithoutKeyArc { transition: t.name.clone() });
            }
            let arcs = t.consumes.iter().chain(&t.inhibits).map(|a| a.place.clone());
            readable.push(arcs.chain(t.reads.iter().cloned()).chain(t.same_key_reads.iter().cloned()).collect());
        }
        let mut auto_order: Vec<TransitionId> = (0..self.transitions.len())
            .filter(|&i| self.transitions[i].trigger == Trigger::Auto)
            .map(TransitionId)
            .collect();
        auto_order.sort_by_key(|id| std::cmp::Reverse(self.transitions[id.0].priority));
        Ok(Net { places, transitions: self.transitions, readable, auto_order })
    }
}

/// A checked net. Bind it to a store with `Net::bind`.
pub struct Net<K: Kinds> {
    pub(crate) places: BTreeMap<K::Place, Discipline>,
    pub(crate) transitions: Vec<Transition<K>>,
    pub(crate) readable: Vec<BTreeSet<K::Place>>,
    /// Priority descending, then declaration order (the sort is stable).
    pub(crate) auto_order: Vec<TransitionId>,
}

impl<K: Kinds> Net<K> {
    pub fn places(&self) -> impl Iterator<Item = (K::Place, Discipline)> + '_ {
        self.places.iter().map(|(p, d)| (p.clone(), *d))
    }

    pub fn transition(&self, id: TransitionId) -> &Transition<K> {
        &self.transitions[id.0]
    }

    pub fn transition_ids(&self) -> impl Iterator<Item = TransitionId> {
        (0..self.transitions.len()).map(TransitionId)
    }
}
