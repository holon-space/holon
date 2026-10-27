//! One commit order across every write store.
//!
//! Each store mints a [`Stamp`] synchronously inside its commit, and the
//! component that hands the commit's rows to the derived views feeds the
//! stamp back. [`CommitClock::low_watermark`] is then the first stamp a view
//! may still be missing: every commit below it, from every source, is fed.
//! A commit minted but never fed holds the watermark forever, so each minting
//! site needs a feeding site.

use std::collections::BTreeSet;
use std::sync::Mutex;

/// A position in the commit order. Minted stamps start at 1; [`Stamp::NONE`]
/// is the high water of a clock that minted nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Stamp(u64);

impl Stamp {
    pub const NONE: Stamp = Stamp(0);

    pub fn get(self) -> u64 {
        self.0
    }
}

/// The store a commit happened in. A feed covers one source only, because
/// each source is drained under its own lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommitSource {
    LoroGlobal,
    LoroLayout,
    Sql,
}

impl CommitSource {
    pub const ALL: [CommitSource; 3] = [Self::LoroGlobal, Self::LoroLayout, Self::Sql];

    fn index(self) -> usize {
        match self {
            Self::LoroGlobal => 0,
            Self::LoroLayout => 1,
            Self::Sql => 2,
        }
    }
}

/// Every commit minted when the ticket was taken. Opaque: a writer holds it
/// and asks the clock whether the views have caught up with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteTicket(Stamp);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{store:?} fed through {cover:?}, past the high water {high_water:?}")]
pub struct CoverAboveHighWater {
    pub store: CommitSource,
    pub cover: Stamp,
    pub high_water: Stamp,
}

#[derive(Debug, Default)]
pub struct CommitClock {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    high: u64,
    outstanding: [BTreeSet<u64>; 3],
}

impl CommitClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// The next stamp, outstanding for `source` until a feed covers it.
    pub fn mint(&self, source: CommitSource) -> Stamp {
        let mut inner = self.inner.lock().unwrap();
        inner.high += 1;
        let stamp = inner.high;
        inner.outstanding[source.index()].insert(stamp);
        Stamp(stamp)
    }

    /// Mark `source`'s stamps up to and including `cover` as fed. Stamps of
    /// other sources stay outstanding. A `cover` past the high water names a
    /// commit that was never minted and changes nothing.
    pub fn feed_through(
        &self,
        source: CommitSource,
        cover: Stamp,
    ) -> Result<(), CoverAboveHighWater> {
        let mut inner = self.inner.lock().unwrap();
        if cover.0 > inner.high {
            return Err(CoverAboveHighWater {
                store: source,
                cover,
                high_water: Stamp(inner.high),
            });
        }
        let set = &mut inner.outstanding[source.index()];
        *set = set.split_off(&(cover.0 + 1));
        Ok(())
    }

    /// The last stamp minted.
    pub fn high_water(&self) -> Stamp {
        Stamp(self.inner.lock().unwrap().high)
    }

    /// The smallest outstanding stamp, or the stamp after the high water when
    /// nothing is outstanding.
    pub fn low_watermark(&self) -> Stamp {
        let inner = self.inner.lock().unwrap();
        let first = inner.outstanding.iter().filter_map(|s| s.first()).min();
        Stamp(first.copied().unwrap_or(inner.high + 1))
    }

    /// `source`'s stamps that no feed covered yet, ascending.
    pub fn outstanding(&self, source: CommitSource) -> Vec<Stamp> {
        let inner = self.inner.lock().unwrap();
        inner.outstanding[source.index()]
            .iter()
            .map(|&s| Stamp(s))
            .collect()
    }

    pub fn ticket(&self) -> WriteTicket {
        WriteTicket(self.high_water())
    }

    /// Whether every commit the ticket covers is fed.
    pub fn is_fed(&self, ticket: WriteTicket) -> bool {
        self.low_watermark() > ticket.0
    }
}

#[cfg(test)]
mod tests;
