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
    /// other sources stay outstanding.
    ///
    /// # Panics
    /// When `cover` is past the high water: the caller names a commit that
    /// was never minted.
    pub fn feed_through(&self, source: CommitSource, cover: Stamp) {
        let mut inner = self.inner.lock().unwrap();
        assert!(
            cover.0 <= inner.high,
            "{source:?} fed through {cover:?}, past the high water {}",
            inner.high
        );
        let set = &mut inner.outstanding[source.index()];
        *set = set.split_off(&(cover.0 + 1));
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
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[derive(Debug, Clone)]
    enum Step {
        Mint(CommitSource),
        /// Feed `source` through the stamp at `pick` percent of the high water.
        Feed(CommitSource, u8),
    }

    fn source() -> impl Strategy<Value = CommitSource> {
        prop_oneof![
            Just(CommitSource::LoroGlobal),
            Just(CommitSource::LoroLayout),
            Just(CommitSource::Sql),
        ]
    }

    fn step() -> impl Strategy<Value = Step> {
        prop_oneof![
            3 => source().prop_map(Step::Mint),
            2 => (source(), 0u8..=100).prop_map(|(s, p)| Step::Feed(s, p)),
        ]
    }

    /// Reference: every minted stamp with its source and whether a feed
    /// covered it.
    #[derive(Default)]
    struct Model {
        minted: Vec<(CommitSource, bool)>,
    }

    impl Model {
        fn high(&self) -> u64 {
            self.minted.len() as u64
        }

        fn outstanding(&self, source: CommitSource) -> Vec<Stamp> {
            self.minted
                .iter()
                .enumerate()
                .filter(|(_, (s, fed))| *s == source && !fed)
                .map(|(i, _)| Stamp(i as u64 + 1))
                .collect()
        }

        fn low(&self) -> Stamp {
            self.minted
                .iter()
                .position(|(_, fed)| !fed)
                .map_or(Stamp(self.high() + 1), |i| Stamp(i as u64 + 1))
        }
    }

    fn assert_agrees(clock: &CommitClock, model: &Model) {
        assert_eq!(clock.high_water(), Stamp(model.high()));
        assert_eq!(clock.low_watermark(), model.low());
        for s in CommitSource::ALL {
            assert_eq!(clock.outstanding(s), model.outstanding(s), "{s:?}");
        }
    }

    proptest! {
        #[test]
        fn interleaved_mints_and_feeds_from_three_sources_keep_the_laws(
            steps in prop::collection::vec(step(), 0..200)
        ) {
            let clock = CommitClock::new();
            let mut model = Model::default();
            let mut low = clock.low_watermark();
            let mut tickets = Vec::new();
            for step in steps {
                match step {
                    Step::Mint(source) => {
                        let stamp = clock.mint(source);
                        prop_assert_eq!(stamp, Stamp(model.high() + 1), "stamps strictly increase");
                        model.minted.push((source, false));
                        tickets.push((clock.ticket(), model.high()));
                    }
                    Step::Feed(source, pick) => {
                        let cover = model.high() * u64::from(pick) / 100;
                        clock.feed_through(source, Stamp(cover));
                        for (i, (s, fed)) in model.minted.iter_mut().enumerate() {
                            if *s == source && i as u64 + 1 <= cover {
                                *fed = true;
                            }
                        }
                    }
                }
                assert_agrees(&clock, &model);
                let now = clock.low_watermark();
                prop_assert!(now >= low, "low watermark went back from {:?} to {:?}", low, now);
                low = now;
                for (ticket, through) in &tickets {
                    let fed = model.minted[..*through as usize].iter().all(|(_, fed)| *fed);
                    prop_assert_eq!(clock.is_fed(*ticket), fed);
                }
            }
            let high = clock.high_water();
            for source in CommitSource::ALL {
                clock.feed_through(source, high);
            }
            prop_assert_eq!(clock.low_watermark(), Stamp(high.get() + 1));
            for source in CommitSource::ALL {
                prop_assert!(clock.outstanding(source).is_empty());
            }
        }
    }

    #[test]
    #[should_panic(expected = "past the high water")]
    fn a_feed_past_the_high_water_panics() {
        let clock = CommitClock::new();
        clock.mint(CommitSource::Sql);
        clock.feed_through(CommitSource::Sql, Stamp(2));
    }
}
