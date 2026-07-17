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
//! @pbt covers the-one-composed-convergence-PBT — random valid wiring over
//! compose_sut, full invariant catalog per tick

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

/// **Large-vault SOAK RUNG — reproduce the full-reseed leak (BugFunnel row 71)
/// at ~4000-block scale.**
///
/// The keystone above boots a 3-block focus doc, where all four `FullReason`
/// leak arms (`empty_pending_moved_frontier`, `unsettled`, `orphan`,
/// `oversized`) fire ZERO times — so the `InvNoSteadyReseedLeak` oracle is a
/// proven-vacuous guard there. This rung inflates the SUT boot to soak scale
/// (`HOLON_SOAK_SEED_BLOCKS`) and replays peer/import transitions — the primary
/// lever for `EmptyPendingMovedFrontier` (a Loro frontier move with no block
/// facts, so `pending` is empty when the projector runs) — so the leak
/// reproduces and can guard the reseed Inc 1-4 fixes.
///
/// Gated on `HOLON_SOAK_RESEED_EXPECT` AND `HOLON_SOAK_SEED_BLOCKS > 0`: unset
/// ⇒ this SKIPS cleanly (disclosed), so it is NEVER part of the default
/// `cargo nextest` run. Hand-rolled (not `prop_state_machine!`) for a runtime
/// skip + deterministic FIXED seeds + interleaved observer reset between boot
/// and replay.
///
/// Deterministic: a hand-built peer-lever sequence over the full-headless
/// oracle (`wide_e2e_ref`, which carries the peer-sync caps). Rather than draw
/// the whole random alphabet — which under-samples the peer lever AND includes
/// UI-gesture transitions (`SutBlockCreate`) that time out resolving a creation
/// slot at 2k+ blocks — each seed replays a FOCUSED
/// `AddPeer → PeerEdit(create on peer) → MergeFromPeer` cycle. `MergeFromPeer`
/// imports the peer's oplog into the primary, moving the Loro frontier WITHOUT
/// producing local commit facts — the exact `EmptyPendingMovedFrontier`
/// condition (`pending` empty, `last != current`) in
/// `loro_sync_controller::project`. The three seeds vary the cycle count
/// (fixed, byte-identical across runs).
#[test]
fn soak_reseed_reproduction() {
    use std::collections::BTreeMap;
    use std::time::Instant;

    use holon_integration_tests::pbt::composed::reseed_observer::ReseedObserver;
    use holon_integration_tests::pbt::composed::soak_seed;
    use holon_integration_tests::pbt::composed::soak_seed::SoakReseedExpect;
    use holon_integration_tests::pbt::composed::wide_e2e::WideE2EMachine;
    use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
    use holon_integration_tests::pbt::transitions::AddPeer;
    use holon_integration_tests::pbt::transitions::E2ETransition;
    use holon_integration_tests::pbt::transitions::MergeFromPeer;
    use holon_integration_tests::pbt::transitions::PeerEdit;
    use holon_integration_tests::pbt::transitions::SyncWithPeer;
    use holon_pbt_core::capabilities::PeerEditOp;
    use proptest_state_machine::ReferenceStateMachine;
    use proptest_state_machine::StateMachineTest;

    // One deterministic peer-lever cycle count per seed (all land in 20..40
    // transitions: 1 + 2·edits + floor(edits/3) SyncWithPeer re-baselines).
    const SEED_EDITS: [usize; 3] = [10, 13, 16];

    // Build the focused peer-lever transition sequence for a seed.
    fn peer_lever_sequence(seed_ix: usize, edits: usize) -> Vec<E2ETransition> {
        let mut v = vec![E2ETransition::AddPeer(AddPeer)];
        for i in 0..edits {
            let sid = format!("peer-{seed_ix}-{i}");
            v.push(E2ETransition::PeerEdit(PeerEdit {
                peer_idx: 0,
                op: PeerEditOp::Create {
                    parent_stable_id: None,
                    content: format!("peer content {seed_ix}-{i}"),
                    stable_id: sid,
                },
            }));
            // Import the peer's new block into the primary — the frontier-move
            // lever (no local commit facts ⇒ empty `pending`).
            v.push(E2ETransition::MergeFromPeer(MergeFromPeer { peer_idx: 0 }));
            // Periodically re-baseline the peer from the primary snapshot so the
            // next create diverges from a fresh common ancestor.
            if i % 3 == 2 {
                v.push(E2ETransition::SyncWithPeer(SyncWithPeer { peer_idx: 0 }));
            }
        }
        v
    }

    let Some(expect) = soak_seed::soak_reseed_expect() else {
        eprintln!(
            "[soak-reseed] SKIP soak_reseed_reproduction: set HOLON_SOAK_RESEED_EXPECT=reproduce \
             (or =zero) AND HOLON_SOAK_SEED_BLOCKS=4000 to reproduce/guard the full-reseed leak \
             (BugFunnel row 71) at vault scale. Unset ⇒ this is not part of the default run."
        );
        return;
    };
    let blocks = soak_seed::soak_block_count();
    if blocks == 0 {
        eprintln!(
            "[soak-reseed] SKIP soak_reseed_reproduction: HOLON_SOAK_RESEED_EXPECT is set but \
             HOLON_SOAK_SEED_BLOCKS is 0 — a soak-scale boot is the whole point. Set \
             HOLON_SOAK_SEED_BLOCKS=4000 (≥2000)."
        );
        return;
    }

    let target = soak_seed::soak_reseed_reason();
    let assert_p95 = soak_seed::soak_assert_p95_ms();
    const SEEDS: usize = 3;
    // Post-boot floor: the soak seeder emits exactly `blocks` block ids, each
    // draining into `block_raw`. Allow 10% slack for the flat boot settle, but a
    // gross under-seed (boot ended unsettled) fails loud below.
    let min_live = blocks * 9 / 10;

    eprintln!(
        "[soak-reseed] mode={expect:?} target_reason={} blocks={blocks} seeds={SEEDS} \
         (deterministic full-headless oracle + focused peer/import lever)",
        target.as_str()
    );

    let mut all_durations_ms: Vec<u128> = Vec::new();
    let mut per_seed_target: Vec<usize> = Vec::new();
    let mut agg_by_reason: BTreeMap<String, usize> = BTreeMap::new();
    let mut agg_steady_total = 0usize;
    let mut kind_counts: BTreeMap<String, usize> = BTreeMap::new();

    for seed_ix in 0..SEEDS {
        // The full-headless oracle carries the peer-sync caps; a fresh one per
        // seed so each boot starts from the same soak-scale baseline.
        let transitions = peer_lever_sequence(seed_ix, SEED_EDITS[seed_ix]);
        eprintln!(
            "[soak-reseed] seed {seed_ix}: booting soak-scale SUT ({blocks} blocks), then \
             replaying {} transitions",
            transitions.len()
        );

        let boot_start = Instant::now();
        let mut ref_state = wide_e2e_ref();
        let mut sut = <ComposedSut<WideE2E> as StateMachineTest>::init_test(&ref_state);
        let live = sut.sut_block_count();
        eprintln!(
            "[soak-reseed] seed {seed_ix}: booted live_blocks={live} in {:?} (floor {min_live})",
            boot_start.elapsed()
        );
        assert!(
            live >= min_live,
            "[soak-reseed] seed {seed_ix}: boot under-seeded — live_blocks={live} < floor \
             {min_live} for HOLON_SOAK_SEED_BLOCKS={blocks}. The boot ended unsettled / the vault \
             never drained; the reproduction would be measuring an empty vault. THIS IS A \
             FINDING (raise HOLON_SOAK_SETTLE_MS or investigate the boot drain)."
        );

        // Boot/seed reseeds (legitimate coldboot) precede this reset; everything
        // after is steady-state and attributed to an interactive transition.
        ReseedObserver::global().reset();

        for t in transitions {
            assert!(
                WideE2EMachine::preconditions(&ref_state, &t),
                "[soak-reseed] seed {seed_ix}: hand-built transition {t:?} violates its \
                 precondition against the current oracle state — the peer-lever sequence is \
                 malformed (fix the constructor, do not skip)."
            );
            ref_state = WideE2EMachine::apply(ref_state, &t);
            let label = format!("{t:?}");
            let kind = label
                .split(['(', ' ', '{'])
                .next()
                .unwrap_or("<?>")
                .to_string();
            *kind_counts.entry(kind).or_default() += 1;
            // Under plain `pbt` (no `otel-testing`) the harness's own
            // `note_transition` is compiled out, so the rung marks the observer
            // steady itself — otherwise every steady leak would be misattributed
            // as boot/seed and never counted.
            ReseedObserver::global().note_transition(&label);
            let t_start = Instant::now();
            sut = <ComposedSut<WideE2E> as StateMachineTest>::apply(sut, &ref_state, t);
            all_durations_ms.push(t_start.elapsed().as_millis());
        }

        let summary = ReseedObserver::global().summary();
        eprintln!(
            "[soak-reseed] seed {seed_ix}: {} | target({})={}",
            summary.report(),
            target.as_str(),
            summary.steady_leak_count_for(target),
        );
        per_seed_target.push(summary.steady_leak_count_for(target));
        agg_steady_total += summary.steady_leak_total;
        for (r, n) in &summary.full_by_reason {
            *agg_by_reason.entry(r.clone()).or_default() += n;
        }
    }

    // p95 wall-clock (nearest-rank), ALWAYS disclosed.
    let p95_ms = {
        let mut d = all_durations_ms.clone();
        d.sort_unstable();
        if d.is_empty() {
            0
        } else {
            let rank = ((d.len() as f64) * 0.95).ceil() as usize;
            d[rank.clamp(1, d.len()) - 1]
        }
    };
    let target_total: usize = per_seed_target.iter().sum();
    let reason_line: Vec<String> = agg_by_reason
        .iter()
        .map(|(r, n)| format!("{r}={n}"))
        .collect();
    let kind_line: Vec<String> = kind_counts
        .iter()
        .map(|(k, n)| format!("{k}={n}"))
        .collect();
    eprintln!(
        "[soak-reseed] SUMMARY across {SEEDS} seeds: target_reason={} target_leaks_per_seed={:?} \
         target_total={target_total} steady_leak_total={agg_steady_total} \
         full_by_reason=[{}] transition_kinds=[{}] p95_action_ms={p95_ms} \
         (n_transitions={})",
        target.as_str(),
        per_seed_target,
        reason_line.join(" "),
        kind_line.join(" "),
        all_durations_ms.len(),
    );

    if let Some(budget) = assert_p95 {
        assert!(
            p95_ms < budget as u128,
            "[soak-reseed] p95 action latency {p95_ms}ms ≥ HOLON_SOAK_ASSERT_P95_MS={budget}ms"
        );
    }

    match expect {
        SoakReseedExpect::Reproduce => {
            assert!(
                target_total >= 1,
                "[soak-reseed] REPRODUCTION FAILED: target reason {} fired 0 steady-state \
                 full-reseeds across all {SEEDS} seeds at {blocks} blocks. This is a CRITICAL \
                 finding — the leak did NOT reproduce even at scale with peer/import transitions \
                 (already fixed, or environment-specific). What DID fire: full_by_reason=[{}] \
                 steady_leak_total={agg_steady_total} transition_kinds=[{}]. Do NOT weaken this \
                 assertion — re-triage the reproduction.",
                target.as_str(),
                reason_line.join(" "),
                kind_line.join(" "),
            );
            eprintln!(
                "[soak-reseed] REPRODUCED: {} fired {target_total} steady-state full-reseed leak(s) \
                 — the rung is a live (non-vacuous) guard for the reseed Inc 1-4 fixes.",
                target.as_str()
            );
        }
        SoakReseedExpect::Zero => {
            assert_eq!(
                agg_steady_total,
                0,
                "[soak-reseed] EXPECTED ZERO leaks (post-fix guard) but observed \
                 steady_leak_total={agg_steady_total} across {SEEDS} seeds: \
                 full_by_reason=[{}]. A reseed Inc 1-4 fix regressed.",
                reason_line.join(" "),
            );
        }
    }
}
