//! **THE SWAP (§5) — `general_e2e` driven over a composed `CapMap`, not `E2ESut`.**
//!
//! The production integration-test entry point for the COMPOSED general-purpose E2E
//! PBT: it drives the production `E2ETransition` enum via the production
//! `aggregate_transitions` generator (auto-narrowed by the `full_headless` wiring +
//! cap_set) over `compose_sut(full_headless())` — assembled by the EXACT
//! [`crate::pbt::composed::builder::compose_sut`] the swap targets — checked by the full
//! composed invariant catalog. This is the North-Star "one composed convergence PBT":
//! the SUT is a per-subsystem composed `CapMap`, not the monolithic `E2ESut`.
//!
//! It runs ALONGSIDE the `E2ESut`-backed [`general_e2e_pbt`](super) for now: the
//! composed alphabet auto-narrows out peer / E4 (PressKey/Arrow/drag) / fixture /
//! seam-mutate transitions + the two deferred-convergence families (watches = B5,
//! `ToggleState` = the Loro-doc-coherence fix), so the `E2ESut` test stays the wide-
//! coverage reference until those converge — at which point it (and its headless cap
//! impls) can be deleted (E3/E5). The slice machinery lives in the `pbt`-gated
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
    fn general_e2e_composed_pbt(sequential 1..8 => ComposedSut<WideE2E>);
}
