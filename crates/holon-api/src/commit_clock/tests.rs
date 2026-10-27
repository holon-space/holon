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
                    clock.feed_through(source, Stamp(cover)).unwrap();
                    for (i, (s, fed)) in model.minted.iter_mut().enumerate() {
                        if *s == source && (i as u64) < cover {
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
            clock.feed_through(source, high).unwrap();
        }
        prop_assert_eq!(clock.low_watermark(), Stamp(high.get() + 1));
        for source in CommitSource::ALL {
            prop_assert!(clock.outstanding(source).is_empty());
        }
    }
}

#[test]
fn a_feed_past_the_high_water_is_refused_and_the_clock_works_on() {
    let clock = CommitClock::new();
    clock.mint(CommitSource::Sql);
    assert_eq!(
        clock.feed_through(CommitSource::Sql, Stamp(2)),
        Err(CoverAboveHighWater {
            store: CommitSource::Sql,
            cover: Stamp(2),
            high_water: Stamp(1),
        })
    );
    assert_eq!(clock.mint(CommitSource::LoroGlobal), Stamp(2));
    assert_eq!(clock.high_water(), Stamp(2));
    assert_eq!(clock.low_watermark(), Stamp(1));
}
