use std::collections::BTreeSet;

use crate::{Kinds, RowView, Store};

/// The store as a bind sees it, without the store's stamp types.
pub(crate) trait Reader<K: Kinds> {
    fn row(&self, place: &K::Place, key: &K::Key) -> RowView<'_, K>;
    fn keys(&self, place: &K::Place) -> Vec<K::Key>;
}

impl<K: Kinds, S: Store<K>> Reader<K> for S {
    fn row(&self, place: &K::Place, key: &K::Key) -> RowView<'_, K> {
        Store::row(self, place, key)
    }

    fn keys(&self, place: &K::Place) -> Vec<K::Key> {
        Store::keys(self, place)
    }
}

/// A bind's window on the marking. It records what it reads; a read of a
/// place the transition did not declare returns `None` and refuses the firing.
pub struct View<'a, K: Kinds> {
    reader: &'a dyn Reader<K>,
    readable: &'a BTreeSet<K::Place>,
    pub(crate) rows: BTreeSet<(K::Place, K::Key)>,
    pub(crate) scans: BTreeSet<K::Place>,
    pub(crate) undeclared: Option<K::Place>,
}

impl<'a, K: Kinds> View<'a, K> {
    pub(crate) fn new(reader: &'a dyn Reader<K>, readable: &'a BTreeSet<K::Place>) -> Self {
        View { reader, readable, rows: BTreeSet::new(), scans: BTreeSet::new(), undeclared: None }
    }

    fn declared(&mut self, place: &K::Place) -> bool {
        let ok = self.readable.contains(place);
        if !ok && self.undeclared.is_none() {
            self.undeclared = Some(place.clone());
        }
        ok
    }

    pub fn row(&mut self, place: &K::Place, key: &K::Key) -> Option<RowView<'a, K>> {
        if !self.declared(place) {
            return None;
        }
        self.rows.insert((place.clone(), key.clone()));
        Some(self.reader.row(place, key))
    }

    /// Keys of the non-empty rows of `place`, ascending.
    pub fn keys(&mut self, place: &K::Place) -> Option<Vec<K::Key>> {
        if !self.declared(place) {
            return None;
        }
        self.scans.insert(place.clone());
        Some(self.reader.keys(place))
    }
}
