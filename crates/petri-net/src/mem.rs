use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::fmt;

use crate::Applied;
use crate::At;
use crate::Discipline;
use crate::Kinds;
use crate::Out;
use crate::RowView;
use crate::Store;

/// A token stamp of the in-memory store: a unique id with no meaningful order,
/// so no layer above can come to rely on one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MemStamp(u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemRowStamp {
    /// The stamps of the row's tokens, head first.
    Tokens(Vec<MemStamp>),
    Count(u32),
}

type TokenRows<K> =
    BTreeMap<(<K as Kinds>::Place, <K as Kinds>::Key), VecDeque<(<K as Kinds>::Token, MemStamp)>>;

/// The marking in memory. Rows that become empty are removed: no tombstones.
pub struct MemStore<K: Kinds> {
    places: BTreeMap<K::Place, Discipline>,
    tokens: TokenRows<K>,
    counts: BTreeMap<(K::Place, K::Key), u32>,
    seed: u64,
    issued: u64,
}

impl<K: Kinds> MemStore<K> {
    /// `seed` shuffles the stamp ids; equal seeds give equal stamps.
    pub fn new(places: impl IntoIterator<Item = (K::Place, Discipline)>, seed: u64) -> Self {
        MemStore {
            places: places.into_iter().collect(),
            tokens: BTreeMap::new(),
            counts: BTreeMap::new(),
            seed,
            issued: 0,
        }
    }

    fn discipline(&self, place: &K::Place) -> Discipline {
        *self
            .places
            .get(place)
            .unwrap_or_else(|| panic!("place {place:?} is not in this store"))
    }

    fn fresh_stamp(&mut self) -> MemStamp {
        self.issued += 1;
        // An odd multiplier and the splitmix64 finalizer are both bijections,
        // so ids stay unique while their order looks random.
        let mut z = self
            .seed
            .wrapping_add(self.issued.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        MemStamp(z ^ (z >> 31))
    }

    fn put(&mut self, place: K::Place, key: K::Key, entry: (K::Token, MemStamp), at: At) {
        let discipline = self.discipline(&place);
        assert!(
            at == At::Back || discipline == Discipline::Fifo,
            "a move to the front of the non-Fifo place {place:?}"
        );
        let row = self.tokens.entry((place, key)).or_default();
        match discipline {
            Discipline::Single => {
                assert!(row.is_empty(), "produce into a full Single row");
                row.push_back(entry);
            }
            Discipline::Register => {
                row.clear();
                row.push_back(entry);
            }
            Discipline::Fifo if at == At::Front => row.push_front(entry),
            Discipline::Fifo => row.push_back(entry),
            Discipline::Counted { .. } => panic!("a token output into a Counted place"),
        }
    }
}

impl<K: Kinds> Clone for MemStore<K> {
    fn clone(&self) -> Self {
        MemStore {
            places: self.places.clone(),
            tokens: self.tokens.clone(),
            counts: self.counts.clone(),
            seed: self.seed,
            issued: self.issued,
        }
    }
}

impl<K: Kinds> fmt::Debug for MemStore<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemStore")
            .field("tokens", &self.tokens)
            .field("counts", &self.counts)
            .finish_non_exhaustive()
    }
}

impl<K: Kinds> Store<K> for MemStore<K> {
    type Stamp = MemStamp;
    type RowStamp = MemRowStamp;

    fn row(&self, place: &K::Place, key: &K::Key) -> RowView<'_, K> {
        let at = (place.clone(), key.clone());
        if self.discipline(place).is_counted() {
            RowView::Count(self.counts.get(&at).copied().unwrap_or(0))
        } else {
            RowView::Tokens(
                self.tokens
                    .get(&at)
                    .map_or(vec![], |r| r.iter().map(|(t, _)| t).collect()),
            )
        }
    }

    fn row_stamp(&self, place: &K::Place, key: &K::Key) -> MemRowStamp {
        let at = (place.clone(), key.clone());
        if self.discipline(place).is_counted() {
            MemRowStamp::Count(self.counts.get(&at).copied().unwrap_or(0))
        } else {
            MemRowStamp::Tokens(
                self.tokens
                    .get(&at)
                    .map_or(vec![], |r| r.iter().map(|(_, s)| *s).collect()),
            )
        }
    }

    fn keys(&self, place: &K::Place) -> Vec<K::Key> {
        let rows = if self.discipline(place).is_counted() {
            self.counts.keys().cloned().collect::<Vec<_>>()
        } else {
            self.tokens.keys().cloned().collect()
        };
        rows.into_iter()
            .filter(|(p, _)| p == place)
            .map(|(_, k)| k)
            .collect()
    }

    fn places(&self) -> Vec<(K::Place, Discipline)> {
        self.places.iter().map(|(p, d)| (p.clone(), *d)).collect()
    }

    fn apply(&mut self, firing: Applied<K>) -> Vec<MemStamp> {
        let mut consumed: Vec<Option<(K::Token, MemStamp)>> =
            Vec::with_capacity(firing.consumes.len());
        for (place, key) in firing.consumes {
            let at = (place, key);
            if self.discipline(&at.0).is_counted() {
                let n = self
                    .counts
                    .get_mut(&at)
                    .expect("consume from an empty Counted row");
                *n -= 1;
                if *n == 0 {
                    self.counts.remove(&at);
                }
                consumed.push(None);
            } else {
                let row = self.tokens.get_mut(&at).expect("consume from an empty row");
                consumed.push(row.pop_front());
                if row.is_empty() {
                    self.tokens.remove(&at);
                }
            }
        }
        let mut stamps = Vec::new();
        for (place, key, out) in firing.outputs {
            match out {
                Out::Unit => {
                    let Discipline::Counted { size } = self.discipline(&place) else {
                        panic!("a unit output into the token place {place:?}");
                    };
                    let n = self.counts.entry((place, key)).or_insert(0);
                    *n += 1;
                    assert!(*n <= size, "a Counted row above its size");
                }
                Out::New(token) => {
                    let stamp = self.fresh_stamp();
                    stamps.push(stamp);
                    self.put(place, key, (token, stamp), At::Back);
                }
                Out::Moved { from, at } => {
                    let entry = consumed[from]
                        .take()
                        .expect("a token moved twice, or a unit moved out of a Counted row");
                    self.put(place, key, entry, at);
                }
            }
        }
        stamps
    }
}
