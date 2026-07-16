//! **THE SWAP (§5) — `general_e2e` driven over a composed `CapMap`, not
//! `E2ESut`.**
//!
//! The production integration-test entry point for the COMPOSED general-purpose
//! E2E PBT — the ONE keystone. Each case DRAWS a valid wiring
//! (`any_valid_wiring()` over `wiring_axes()`, shrinking toward Loro-only) and
//! boots `compose_sut(set_for_wiring(w))` — assembled by the EXACT
//! [`crate::pbt::composed::builder::compose_sut`] production builder — then
//! drives the production `E2ETransition` enum via the production
//! `aggregate_transitions` generator, auto-narrowed by the drawn wiring + its
//! composed cap_set, checked by the full composed invariant catalog gated per
//! draw by `required_invariants` (the non-vacuity floor). This is the
//! North-Star "one composed convergence PBT": untested-wiring bugs are the
//! point — a wiring either draws-and-tests here, fails loud at composition time
//! (`Wiring::validate` /
//! `OperationDispatcher::assert_content_write_capability`), or is a conscious
//! omission from `wiring_axes()`.
//!
//! Each case discloses its draw on stderr (`[wide-e2e wiring] ...`), so a run
//! log yields per-wiring case counts. `HOLON_PBT_FORCE_FULL=1` pins every case
//! to `full_headless`; `HOLON_PBT_WIRING_AXES="storage;sync;actors"` scopes the
//! drawn axes. See docs/Testing/PBT.md §"The wiring grid". The slice machinery
//! lives in the `pbt`-gated [`crate::pbt::composed::wide_e2e`] module (the
//! single source of truth, also driven by the lib `frontend_wide_pbt` + teeth);
//! this file is just the integration entry point.
//!
//! @pbt kind keystone
//! @pbt covers the-one-composed-convergence-PBT — random valid wiring over compose_sut, full invariant catalog per tick

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::live_mcp::LiveMcpE2E;
use holon_integration_tests::pbt::composed::live_mcp::WideE2ELiveMcpMachine;
use holon_integration_tests::pbt::composed::live_mcp::capture_live_cap_set;
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

/// The out-of-process twin of `general_e2e_composed_pbt`: the SAME keystone
/// transitions + invariant catalog, driven against a LIVE Holon app over MCP at
/// `http://127.0.0.1:$MCP_SERVER_PORT/mcp` (default 8521), with a per-case
/// in-process `reset_vault`.
///
/// Gated on `HOLON_PBT_LIVE_MCP`: unset ⇒ this SKIPS cleanly (disclosed) so the
/// headless keystone above is the only PBT a plain `cargo test` runs.
/// Hand-rolled (not `prop_state_machine!`) because that macro cannot express a
/// runtime skip.
///
/// `max_shrink_iters: 0` — NEVER shrink through resets (the server's reset
/// budget is 20/process). To reproduce a live-found failure: replay its
/// persisted regression seed in-proc (`HOLON_PBT_FORCE_FULL=1 cargo test
/// general_e2e_composed_pbt`, the headless twin, which shares the alphabet) to
/// shrink it, then re-verify the shrunk seed live once against a fresh app.
#[test]
fn general_e2e_composed_pbt_live_mcp() {
    use proptest::test_runner::Config;
    use proptest::test_runner::TestRunner;
    use proptest_state_machine::ReferenceStateMachine;
    use proptest_state_machine::StateMachineTest;

    if std::env::var("HOLON_PBT_LIVE_MCP").is_err() {
        eprintln!(
            "[live-mcp] SKIP general_e2e_composed_pbt_live_mcp: set HOLON_PBT_LIVE_MCP=1 \
             (and run a Holon app serving MCP at http://127.0.0.1:$MCP_SERVER_PORT/mcp with \
             HOLON_MCP_ALLOW_RESET=1) to drive the composed keystone over a live app."
        );
        return;
    }

    // Capture the live cap set (throwaway connect, no reset) BEFORE the strategy is
    // built, then drop that runtime — proptest runs the state machine synchronously
    // with no ambient runtime, and each case's `init_test` builds its own.
    capture_live_cap_set();

    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    let config = Config {
        cases,
        max_shrink_iters: 0,
        ..Config::default()
    };
    let strategy = <WideE2ELiveMcpMachine as ReferenceStateMachine>::sequential_strategy(1..40);
    let mut runner = TestRunner::new(config.clone());
    let result = runner.run(&strategy, |(initial_state, transitions, seen_counter)| {
        <ComposedSut<LiveMcpE2E> as StateMachineTest>::test_sequential(
            config.clone(),
            initial_state,
            transitions,
            seen_counter,
        );
        Ok(())
    });
    result.expect("live-mcp composed keystone diverged from the oracle");
}
