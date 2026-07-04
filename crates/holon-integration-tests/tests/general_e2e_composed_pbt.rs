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
        failure_persistence: None,
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
        failure_persistence: None,
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

/// **Vault-scale NAVIGATION-LATENCY soak rung — reproduce the cold
/// focus-descendant matview cliff (BugFunnel row 78) at ~2000-block scale.**
///
/// The keystone boots a 3-block focus doc, so the whole-vault projection cost
/// of a page navigation never manifests — Martin only meets it by hand at real
/// vault scale (`navigate focus` measured 2674ms @4000 blocks; the cold
/// focus-descendant `watch_view` seeds its recursive descendant traversal from
/// ALL blocks and applies the focus filter LAST, so the first visit to any root
/// pays an O(vault) materialization: ~11.9s @1038 live blocks). This rung
/// inflates the SUT boot to soak scale (`HOLON_SOAK_SEED_BLOCKS`) and drives
/// REAL page navigations through the EXACT keystone drivers, measuring
/// wall-clock dispatch→settled per navigation as TWO steps:
///   1. `SutFocusWrite::apply_navigate_focus` — the sidebar click-to-focus path
///      (`click_entity(left_sidebar) -> navigation.focus intent -> nav SQL
///      write
///      + focus-matview settle`). This writes `focus_roots`/`current_focus` — a
///      ~sub-second focus write, the "warm" baseline.
///   2. `SutWatchRegister::register_watch` (the `SetupWatch` transition, driven
///      through the SAME production `ReactiveEngine::watch_query_live` path the
///      real UI's main panel uses) — registers the main-panel focus-descendant
///      CONTENT watch and settles it. This is the O(vault) content
///      materialization = the RC5 cliff locus.
///
/// **Why step 2 is required.** `settle_focus_matviews` (inside NavigateFocus)
/// polls ONLY the lightweight `current_focus`/`focus_roots` tables; the
/// headless harness never registers the main-panel descendant watch (boot
/// registers only the sidebar page-list watch), so a bare NavigateFocus is
/// STRUCTURALLY BLIND to the cold materialization — the "make E2E and prod more
/// similar" coldness gap. Step 2 closes it by registering the exact watch the
/// main panel does.
///
/// Focus roots: the deterministic synthetic soak DOC pages
/// (`block:soak-doc-0 .. block:soak-doc-{n-1}`, `n = soak_doc_count()`), each a
/// real ingested page that renders in the LeftSidebar. "First navigation after
/// boot" is cold by construction: boot seeds focus on `block:journals`, so the
/// user's first navigation to any soak page is a never-visited root that must
/// materialize its descendant content from scratch.
///
/// Gated on `HOLON_SOAK_NAV` (`reproduce` | `zero`) AND `HOLON_SOAK_SEED_BLOCKS
/// > 0`: unset ⇒ this SKIPS cleanly (disclosed), so it is NEVER part of the
/// default `cargo nextest` run. Hand-rolled (not `prop_state_machine!`) for a
/// runtime skip + deterministic FIXED navigation plan.
///
/// **THE LEVER IS DEPTH, NOT COUNT.** RC5 is driven by the recursive-CTE cycle
/// guard (`','||visited||',' NOT LIKE '%,'||id||',%'`) — an O(path-length)
/// string scan PER recursion step, so cost is ~quadratic in tree DEPTH.
/// Measured (release): `wide` (2000 shallow blocks, depth ≤4) = 271ms cold
/// descendant-matview (does NOT reproduce); `deep` (1000 blocks, 5 chains ×
/// depth 200) = **19,029ms** cold (reproduces, worse than the prod
/// ~11.9s@1038); `mixed` = 1,556ms. So the reproducing invocation MUST set
/// `HOLON_SOAK_SHAPE=deep` (see
/// [`crate::pbt::composed::soak_seed::SoakShape`]).
///
/// **INVOCATION (RELEASE profile — wall-clock is meaningless un-optimized).**
/// The `--test general_e2e_composed_pbt` filter is REQUIRED — without it every
/// integration-test binary in the package release-compiles (minutes of wasted
/// CPU). `pbt` is a DEFAULT feature of `holon-integration-tests`, so no
/// `--features` flag is needed for this headless rung (`holon-gpui/pbt` is not
/// a feature of this package and would error).
/// Red-first proof (reports the breach; PASS = cliff reproduced):
/// ```text
/// HOLON_SOAK_NAV=reproduce HOLON_SOAK_SHAPE=deep HOLON_SOAK_SEED_BLOCKS=1000 \
///   HOLON_SOAK_DEPTH=200 HOLON_SOAK_SETTLE_MS=30000 \
///   cargo nextest run -p holon-integration-tests --test general_e2e_composed_pbt \
///   --release --no-capture -E 'test(soak_nav_latency)' 2>&1 | tee /tmp/soak-nav.log
/// ```
/// Demonstrate the guard going RED (SLO breach, content p95 ≈ 19s ≫ 2s): add
/// `HOLON_SOAK_NAV_ASSERT=1` to the `deep` invocation. Post-fix regression
/// guard: `HOLON_SOAK_NAV=zero`.
///
/// @pbt kind soak
/// @pbt covers cold-focus-descendant-matview-latency — first-visit navigation
/// pays O(vault) materialization
#[test]
fn soak_nav_latency() {
    use std::time::Instant;

    use holon_api::EntityUri;
    use holon_api::QueryLanguage;
    use holon_api::Region;
    use holon_integration_tests::pbt::composed::soak_seed;
    use holon_integration_tests::pbt::composed::soak_seed::SoakNavExpect;
    use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
    use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
    use holon_integration_tests::pbt::query::QuerySource;
    use holon_integration_tests::pbt::query::QueryTable;
    use holon_integration_tests::pbt::query::TestQuery;
    use holon_integration_tests::pbt::transitions::E2ETransition;
    use holon_integration_tests::pbt::transitions::NavigateFocus;
    use holon_integration_tests::pbt::transitions::SetupWatch;
    use proptest_state_machine::StateMachineTest;

    // Reproduction bar for the RC5 cliff: the focus-descendant matview
    // materialization p95 must be multi-second to count as "the cliff
    // reproduced". Prod measured ~11.9s @1038 live blocks; 1500ms is a generous,
    // host-robust floor well above the sub-second materialization the harness
    // shows at 2000 blocks but far below the prod cliff — so it distinguishes
    // "cliff present" from "materialization is fast here" without host flakiness.
    const CLIFF_FLOOR_MS: u128 = 1500;
    // Cap on how many distinct soak pages to drive as cold roots (keeps a
    // 2000-block release run to a few minutes — ~10 cold materializations).
    const MAX_ROOTS: usize = 12;

    let Some(expect) = soak_seed::soak_nav_expect() else {
        eprintln!(
            "[soak-nav] SKIP soak_nav_latency: set HOLON_SOAK_NAV=reproduce (or =zero) AND \
             HOLON_SOAK_SEED_BLOCKS=2000 to reproduce/guard the cold focus-descendant matview \
             navigation cliff (BugFunnel row 78) at vault scale. Unset ⇒ not part of the default \
             run. RELEASE profile only (wall-clock)."
        );
        return;
    };
    let blocks = soak_seed::soak_block_count();
    let doc_count = soak_seed::soak_doc_count();
    if blocks == 0 || doc_count == 0 {
        eprintln!(
            "[soak-nav] SKIP soak_nav_latency: HOLON_SOAK_NAV is set but HOLON_SOAK_SEED_BLOCKS \
             is 0 — a soak-scale boot is the whole point. Set HOLON_SOAK_SEED_BLOCKS=2000 \
             (≥2000, yielding ≥1 navigable soak page)."
        );
        return;
    }

    let roots = doc_count.min(MAX_ROOTS);
    let budget = soak_seed::soak_nav_budget_ms();
    let hard_assert = soak_seed::soak_nav_hard_assert();
    // Post-boot floor: the soak seeder emits exactly `blocks` block ids draining
    // into `block_raw`; allow 10% slack for the flat boot settle. A gross
    // under-seed (boot ended unsettled) means we'd be measuring an empty vault —
    // fail loud below.
    let min_live = blocks * 9 / 10;

    eprintln!(
        "[soak-nav] mode={expect:?} blocks={blocks} navigable_roots={roots} budget_ms={budget} \
         hard_assert={hard_assert} (deterministic sidebar-click nav via SutFocusWrite)"
    );

    // Boot ONE soak-scale SUT. The oracle is static: `NavigateFocus` mints no
    // blocks, so the harness per-tick reconcile is a 0==0 no-op regardless of
    // the oracle state, and the soak pages need not be modelled by it (the SUT
    // store holds them as real ingested pages; the sidebar-click driver targets
    // them directly). We drive the SUT, not the oracle — this rung measures
    // latency, it does not run the invariant catalog.
    let boot_start = Instant::now();
    let ref_state = wide_e2e_ref();
    let mut sut = <ComposedSut<WideE2E> as StateMachineTest>::init_test(&ref_state);
    let live = sut.sut_block_count();
    eprintln!(
        "[soak-nav] booted live_blocks={live} in {:?} (floor {min_live})",
        boot_start.elapsed()
    );
    assert!(
        live >= min_live,
        "[soak-nav] boot under-seeded — live_blocks={live} < floor {min_live} for \
         HOLON_SOAK_SEED_BLOCKS={blocks}. The boot ended unsettled / the vault never drained; the \
         reproduction would be measuring an empty vault. THIS IS A FINDING (raise \
         HOLON_SOAK_SETTLE_MS or investigate the boot drain)."
    );

    let soak_page = |k: usize| -> EntityUri {
        EntityUri::parse(&format!("block:soak-doc-{k}")).expect("valid soak page id")
    };

    // The PROD main-panel content query (`assets/default/index.org`,
    // `block:default-main-panel`): the navigation-aware recursive descendant of
    // the current `focus_roots(region=main)`. `settle_focus_matviews` (inside
    // NavigateFocus) polls ONLY the lightweight `current_focus`/`focus_roots`
    // tables — it NEVER materializes this main-panel content watch, so a bare
    // NavigateFocus is structurally BLIND to the cold materialization. So after
    // each nav we register the SAME watch the real UI's main panel registers,
    // through the production `ReactiveEngine::watch_query_live` path (the
    // `SutWatchRegister` cap the `SetupWatch` transition binds), and measure its
    // register→settle wall-clock. NB the prod holon_sql form correctly SEEDS
    // from `focus_roots`; the all-blocks recursive scan is introduced by Turso
    // IVM's recursive-CTE compilation of `focus_descendants`. `TestQuery::to_sql`
    // has no recursive surface, so we register the equivalent GQL form
    // (`MATCH (fr:focus_root),(root:block)<-[:CHILD_OF*0..N]-(d) …`), which the
    // query-equivalence PBT proves selects the identical row set and which IVM
    // compiles to the same recursive descendant matview — the cliff locus.
    let main_panel_watch = || TestQuery {
        table: QueryTable::Blocks,
        columns: vec!["id".to_string()],
        predicates: vec![],
        source: QuerySource::FocusRootDescendants {
            // Deep enough to fully descend the `deep`-shape chains (the prod
            // main-panel CTE is unbounded); 512 covers HOLON_SOAK_DEPTH ≤ 512.
            region: "main".to_string(),
            max_depth: 512,
            stop_at_pages: false,
        },
    };

    // Per fresh root: NavigateFocus (the focus write — the ~sub-second baseline)
    // THEN register the main-panel descendant watch (the O(vault) content
    // materialization). "cold" = the full nav→content-visible latency
    // (nav + descendant materialization); "warm" = the focus-write-only baseline
    // (what a revisit with a cached view costs). `watch_ms` isolates the pure
    // descendant materialization — the RC5 cliff, if present.
    let mut cold_ms: Vec<u128> = Vec::new(); // nav + descendant materialization
    let mut warm_ms: Vec<u128> = Vec::new(); // nav (focus write) only
    let mut watch_ms: Vec<u128> = Vec::new(); // descendant materialization only
    let mut all_ms: Vec<u128> = Vec::new();

    for k in 0..roots {
        let root = soak_page(k);
        let nav = E2ETransition::NavigateFocus(NavigateFocus {
            region: Region::Main,
            block_id: root.clone(),
        });
        let s = Instant::now();
        sut = <ComposedSut<WideE2E> as StateMachineTest>::apply(sut, &ref_state, nav);
        let nav_ms = s.elapsed().as_millis();

        let watch = E2ETransition::SetupWatch(SetupWatch {
            query_id: format!("main-panel-{k}"),
            query: main_panel_watch(),
            language: QueryLanguage::HolonGql,
        });
        let s = Instant::now();
        sut = <ComposedSut<WideE2E> as StateMachineTest>::apply(sut, &ref_state, watch);
        let w_ms = s.elapsed().as_millis();

        let content_ms = nav_ms + w_ms;
        warm_ms.push(nav_ms);
        watch_ms.push(w_ms);
        cold_ms.push(content_ms);
        all_ms.push(content_ms);
        eprintln!(
            "[soak-nav] nav #{k} root={root} nav_ms={nav_ms} descendant_watch_ms={w_ms} \
             content_ms={content_ms}"
        );
    }

    // p50/p95 (nearest-rank), ALWAYS disclosed.
    let pct = |data: &[u128], p: f64| -> u128 {
        if data.is_empty() {
            return 0;
        }
        let mut d = data.to_vec();
        d.sort_unstable();
        let rank = ((d.len() as f64) * p).ceil() as usize;
        d[rank.clamp(1, d.len()) - 1]
    };
    // `warm_ms` holds the per-nav focus-write (NavigateFocus) times; `watch_ms`
    // the per-nav focus-descendant materialization (the cliff locus); `cold_ms`
    // / `all_ms` the full nav→content-visible latency (nav + descendant).
    let nav_p50 = pct(&warm_ms, 0.50);
    let nav_p95 = pct(&warm_ms, 0.95);
    let watch_p50 = pct(&watch_ms, 0.50);
    let watch_p95 = pct(&watch_ms, 0.95);
    let watch_max = watch_ms.iter().copied().max().unwrap_or(0);
    let content_p50 = pct(&all_ms, 0.50);
    let content_p95 = pct(&all_ms, 0.95);
    let content_max = all_ms.iter().copied().max().unwrap_or(0);

    eprintln!(
        "[soak-nav] SUMMARY blocks={blocks} roots={roots} n_nav={} \
         | FOCUS-WRITE(nav) p50={nav_p50}ms p95={nav_p95}ms \
         | DESCENDANT-MATVIEW(cliff locus) p50={watch_p50}ms p95={watch_p95}ms max={watch_max}ms \
         | CONTENT(nav→visible) p50={content_p50}ms p95={content_p95}ms max={content_max}ms \
         | budget={budget}ms cliff_floor={CLIFF_FLOOR_MS}ms",
        all_ms.len(),
    );
    eprintln!(
        "[soak-nav] content_ms_per_root={cold_ms:?} descendant_matview_ms_per_root={watch_ms:?}"
    );

    // Opt-in hard assertion (mode-independent): the user-facing content p95
    // (nav→visible) must clear the budget. This is the SLO guard; RED whenever a
    // navigation breaches, GREEN otherwise.
    if hard_assert {
        assert!(
            content_p95 < budget as u128,
            "[soak-nav] HOLON_SOAK_NAV_ASSERT: p95 nav→content-visible latency {content_p95}ms ≥ \
             budget {budget}ms at {blocks} blocks (descendant-matview p95={watch_p95}ms). A \
             navigation breaches the SLO."
        );
    }

    match expect {
        SoakNavExpect::Reproduce => {
            // The RC5 cliff lives in the focus-descendant matview materialization
            // (prod: ~11.9s @1038 live blocks). "Reproduced" = that materialization
            // is multi-second (≥ CLIFF_FLOOR). Keyed on the descendant matview, NOT
            // the nav-dominated content total, so it tests the actual cliff locus.
            let reproduced = watch_p95 >= CLIFF_FLOOR_MS;
            assert!(
                reproduced,
                "[soak-nav] REPRODUCTION FAILED: the cold focus-descendant matview cliff did NOT \
                 reproduce at {blocks} blocks across {roots} fresh roots. The main-panel descendant \
                 watch (registered through the SAME production `watch_query_live` path the real UI \
                 uses) materialized in p95={watch_p95}ms / max={watch_max}ms — FAR below the \
                 multi-second floor ({CLIFF_FLOOR_MS}ms) and ~2 orders of magnitude below the prod \
                 ~11.9s @1038 blocks. This is a HEADLINE FINDING, NOT a bug in this rung: the cliff \
                 is NOT explained by vault scale alone under settle-to-quiescence. Like the reseed \
                 soak rung (BugFunnel row 78/71), the live-vault cost is almost certainly \
                 concurrency-specific (CRDT reprojection / IVM under concurrent readers / real \
                 many-file boot O(N²)), which the serialized harness does not exercise. Do NOT \
                 weaken this assertion — the next probe needs a concurrent-reader-during-navigation \
                 lever or a live-vault measurement, not more blocks. \
                 (descendant_matview_ms_per_root={watch_ms:?})"
            );
            eprintln!(
                "[soak-nav] REPRODUCED: focus-descendant matview p95={watch_p95}ms ≥ \
                 {CLIFF_FLOOR_MS}ms at {blocks} blocks — the O(vault) cold materialization cliff is \
                 live. The rung is a non-vacuous guard for the fix (scope the recursive CTE to the \
                 focus subtree)."
            );
        }
        SoakNavExpect::Zero => {
            assert!(
                content_p95 < budget as u128,
                "[soak-nav] EXPECTED FAST NAV (post-fix guard) but nav→content-visible \
                 p95={content_p95}ms ≥ budget {budget}ms at {blocks} blocks (descendant-matview \
                 p95={watch_p95}ms, max={watch_max}ms). A first-visit navigation regressed into \
                 O(vault) materialization. (content_ms_per_root={cold_ms:?})"
            );
            eprintln!(
                "[soak-nav] GUARD GREEN: nav→content-visible p95={content_p95}ms < budget \
                 {budget}ms — navigation (incl. focus-descendant materialization) stays under SLO."
            );
        }
    }
}
