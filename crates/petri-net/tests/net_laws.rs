use petri_net::MemStore;
use petri_net::testkit::TestKinds;
use petri_net::testkit::arb_net_case;
use petri_net::testkit::check_atomicity;
use petri_net::testkit::check_footprint;
use petri_net::testkit::check_settle_laws;
use petri_net::testkit::take_fifo_moves_into_non_empty;
use proptest::prelude::*;
use proptest::test_runner::Config;
use proptest::test_runner::TestRng;
use proptest::test_runner::TestRunner;

fn mem(places: &[(u8, petri_net::Discipline)]) -> MemStore<TestKinds> {
    MemStore::new(places.iter().copied(), 11)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn a_firing_applies_all_outputs_or_none(case in arb_net_case()) {
        check_atomicity(mem, &case)?;
    }

    #[test]
    fn the_footprint_holds_every_row_an_evaluation_touched(case in arb_net_case()) {
        check_footprint(mem, &case)?;
    }

    #[test]
    fn settle_is_level_dead_crash_safe_and_deterministic(case in arb_net_case()) {
        check_settle_laws(mem, &case)?;
    }
}

/// The atomicity law proves little for Front unless drawn nets move tokens
/// into Fifo rows that already hold tokens, both to the front and the back.
#[test]
fn the_generator_moves_into_non_empty_fifo_rows() {
    let config = Config {
        cases: 512,
        failure_persistence: None,
        ..Config::default()
    };
    // A fixed seed: the count must not depend on the draw of the day.
    let rng = TestRng::deterministic_rng(config.rng_algorithm);
    let mut runner = TestRunner::new_with_rng(config, rng);
    take_fifo_moves_into_non_empty();
    runner
        .run(&arb_net_case(), |case| check_atomicity(mem, &case))
        .unwrap();
    let (front, back) = take_fifo_moves_into_non_empty();
    eprintln!("moves into non-empty Fifo rows: front {front}, back {back}");
    assert!(front > 0 && back > 0, "front {front}, back {back}");
}
