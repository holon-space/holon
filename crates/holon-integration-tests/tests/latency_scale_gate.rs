#![cfg(feature = "pbt")]
//! **The scale axis of the latency gate — cold navigation over a
//! few-thousand-block vault.**
//!
//! Every other latency rung runs at the keystone's ~33-block scale, where the
//! cost this rung exists to score is free. Navigation mints one materialized
//! view per first-visited block and the mint walks the block table
//! recursively, so its cost grows with the whole vault. Measured on the real
//! vault: 1108 ms per navigation at 3126 blocks against 12 ms at 33 blocks
//! (BugFunnel `2026-09-19-navigation-costs-six-seconds-on-the-real-vault`).
//! The missing axis is corpus size, not a missing assertion, which is why this
//! is a new rung rather than a tightened ceiling on an existing one.
//!
//! ## What it drives
//!
//! The workload is the hand-authored corpus
//! `hand-authored-regressions/latency-scale.jsonl` — a fixed transition list,
//! no RNG, so the sample population is identical every run and a ratchet-style
//! ceiling can be compared against it. Each of its navigations goes to a
//! DISTINCT never-visited soak page, because a repeat visit re-uses a cached
//! view and is blind to the cost. Each navigation is driven as two
//! transitions, `NavigateFocus` then `SetupWatch`; the corpus header explains
//! why a bare `NavigateFocus` never materializes the content the user waits
//! for.
//!
//! ## What scores it
//!
//! `e2e.p50.navigate`, the prod correlator's interaction-to-visible stage at
//! `origin=ui` — the quantity the SLO names. The harness's own `action_total`
//! is printed but gates nothing, and one reason is worth knowing before
//! reading a run: a watch registration RETURNS BEFORE its view is minted, so
//! the `CREATE MATERIALIZED VIEW` lands after the `SetupWatch` window closes
//! and inside the next transition's. `SetupWatch` therefore reads tens of
//! milliseconds while the mint it caused runs for a second just outside it.
//! `scripts/latency/mint_attribution.py` scores those mints on their own
//! events; the ceilings file argues the rest.
//!
//! ## Why it does not run the invariant catalog
//!
//! This rung drives the SUT, not the oracle. The soak seeder folds its
//! synthetic pages into the oracle as seed blocks for the block comparison,
//! but the oracle models none of their task states and the ViewModel
//! invariants see their ids as phantoms, so the composed catalog reds on the
//! seed itself rather than on anything the workload did. `soak_nav_latency` in
//! the keystone binary makes the same split for the same reason. Correctness
//! at scale is a separate, unclaimed axis; making the catalog scale-clean is a
//! change to the seeder's oracle folding, not to this rung.
//!
//! ## Invocation
//!
//! `just latency-scale-gate` — it seeds the vault, runs this test, and hands
//! the log to `scripts/measure_latency.py` for judging against
//! `docs/Testing/latency-scale-ceilings.txt`. Without
//! `HOLON_SOAK_SEED_BLOCKS` the test SKIPS cleanly, so it is never part of a
//! default `cargo nextest` run.
//!
//! @pbt kind soak
//! @pbt covers cold-navigation-latency-at-vault-scale — first visit to a block
//! pays an O(vault) materialized-view mint on the interaction path

use std::path::PathBuf;

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::soak_seed;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::hand_authored::parse_case;
use holon_integration_tests::pbt::transitions::E2ETransition;
use proptest_state_machine::StateMachineTest;

/// The workload, relative to the crate root. Fixed rather than env-selected:
/// the ceilings this run is judged against are meaningless over a different
/// transition list.
const CORPUS: &str = "hand-authored-regressions/latency-scale.jsonl";

#[test]
fn latency_scale_cold_navigation() {
    let seeded = soak_seed::soak_block_count();
    if seeded == 0 {
        eprintln!(
            "[latency-scale] SKIP latency_scale_cold_navigation: set HOLON_SOAK_SEED_BLOCKS (and \
             HOLON_SOAK_BLOCKS_PER_DOC) to boot a scale corpus, or run `just \
             latency-scale-gate`. Unset ⇒ not part of the default run."
        );
        return;
    }

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(CORPUS);
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("latency-scale corpus {path:?} is unreadable: {e}"));
    let cases: Vec<_> = raw
        .lines()
        .enumerate()
        .map(|(i, line)| (i + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'))
        .map(|(lineno, line)| parse_case(&path, lineno, line))
        .collect();
    assert_eq!(
        cases.len(),
        1,
        "latency-scale corpus {path:?} must hold exactly one case; found {}",
        cases.len()
    );
    let case = cases.into_iter().next().expect("one case");

    // The corpus names its focus roots by index, so the seed must supply at
    // least that many pages. Checked before the boot: a missing page would
    // otherwise surface deep inside a driver, minutes later.
    let roots = navigated_soak_pages(&case.transitions);
    let available = soak_seed::soak_doc_count();
    assert!(
        !roots.is_empty(),
        "latency-scale corpus navigates to no soak page — the workload would measure nothing"
    );
    let highest = *roots.iter().max().expect("non-empty");
    assert!(
        highest < available,
        "latency-scale corpus navigates to block:soak-doc-{highest} but the seed supplies only \
         {available} pages (HOLON_SOAK_SEED_BLOCKS={seeded}, \
         HOLON_SOAK_BLOCKS_PER_DOC). Raise the seed or lower the corpus."
    );
    eprintln!(
        "[latency-scale] case {:?}: {} transitions, {} distinct cold roots, {available} soak \
         pages seeded from {seeded} blocks",
        case.name,
        case.transitions.len(),
        roots.len(),
    );

    // One boot, then the fixed plan. `apply` is the same production path the
    // keystone drives and it emits the `stage=action_total` event every rung in
    // the ceilings file is scored on.
    let ref_state = wide_e2e_ref();
    let mut sut = <ComposedSut<WideE2E> as StateMachineTest>::init_test(&ref_state);
    for transition in case.transitions {
        sut = <ComposedSut<WideE2E> as StateMachineTest>::apply(sut, &ref_state, transition);
    }
}

/// The soak-page indices the workload navigates to, deduplicated. A repeat
/// visit is not a cold root, so the count is what the rung can honestly claim.
fn navigated_soak_pages(transitions: &[E2ETransition]) -> std::collections::BTreeSet<usize> {
    transitions
        .iter()
        .filter_map(|t| match t {
            E2ETransition::NavigateFocus(nav) => nav
                .block_id
                .as_str()
                .strip_prefix("block:soak-doc-")
                .map(|k| {
                    k.parse::<usize>().unwrap_or_else(|e| {
                        panic!("latency-scale corpus has a malformed soak page id {k:?}: {e}")
                    })
                }),
            _ => None,
        })
        .collect()
}
