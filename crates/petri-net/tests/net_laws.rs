use petri_net::MemStore;
use petri_net::testkit::TestKinds;
use petri_net::testkit::arb_net_case;
use petri_net::testkit::check_atomicity;
use petri_net::testkit::check_footprint;
use petri_net::testkit::check_settle_laws;
use proptest::prelude::*;

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
