//! **THE SWAP (§5) — `general_e2e` driven over a composed `CapMap`, not `E2ESut`.**
//!
//! The production integration-test entry point for the COMPOSED general-purpose E2E
//! PBT — the ONE keystone. Each case DRAWS a valid wiring (`any_valid_wiring()` over
//! `wiring_axes()`, shrinking toward Loro-only) and boots
//! `compose_sut(set_for_wiring(w))` — assembled by the EXACT
//! [`crate::pbt::composed::builder::compose_sut`] production builder — then drives the
//! production `E2ETransition` enum via the production `aggregate_transitions` generator,
//! auto-narrowed by the drawn wiring + its composed cap_set, checked by the full composed
//! invariant catalog gated per draw by `required_invariants` (the non-vacuity floor).
//! This is the North-Star "one composed convergence PBT": untested-wiring bugs are the
//! point — a wiring either draws-and-tests here, fails loud at composition time
//! (`Wiring::validate` / `OperationDispatcher::assert_content_write_capability`), or is
//! a conscious omission from `wiring_axes()`.
//!
//! Each case discloses its draw on stderr (`[wide-e2e wiring] ...`), so a run log yields
//! per-wiring case counts. `HOLON_PBT_FORCE_FULL=1` pins every case to `full_headless`;
//! `HOLON_PBT_WIRING_AXES="storage;sync;actors"` scopes the drawn axes. See
//! docs/Testing/PBT.md §"The wiring grid". The slice machinery lives in the `pbt`-gated
//! [`crate::pbt::composed::wide_e2e`] module (the single source of truth, also driven by
//! the lib `frontend_wide_pbt` + teeth); this file is just the integration entry point.

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use proptest_state_machine::prop_state_machine;

prop_state_machine! {
    #![proptest_config(proptest::test_runner::Config {
        cases: std::env::var("PROPTEST_CASES").ok().and_then(|s| s.parse().ok()).unwrap_or(16),
        max_shrink_iters: 200,
        .. proptest::test_runner::Config::default()
    })]
    #[test]
    fn general_e2e_composed_pbt(sequential 1..40 => ComposedSut<WideE2E>);
}
