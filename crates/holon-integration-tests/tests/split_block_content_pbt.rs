//! `split_block_content_pbt` — SplitBlock content-routing regression, now
//! driven through the composed `ComposedSut<WideE2E>` (E3 migration).
//!
//! # Bug class A (SqlOnly variant)
//!
//! After `SplitBlock(block_id, position)` on a parser-created text block,
//! the SQL projection assigns the **whole pre-split content** to the new
//! block and leaves the original block's `content` empty. The reference
//! model correctly splits prefix→original / suffix→new. Concrete signature
//! from the wide PBT:
//!
//!     ref:  block:k3a7-p="M",  block:a16cd1e0="z1 a3IZ5gmrU"
//!     sql:  block:k3a7-p="",   block:a16cd1e0="Mz1 a3IZ5gmrU"
//!
//! Surfaced and landed as test-infra fix May 2026 — see MEMORY
//! `sqlonly_splitblock_cursor_bug_2026-05-19`.
//!
//! # Bug class B (Full variant)
//!
//! Production splits fail with `Cannot resolve parent URI to TreeID`
//! when the parent block was created via org-file ingestion. The Loro
//! inbound runtime never acks any `block.*` events
//! (`event_acks.consumer='loro'` empty), so SQL→Loro replay never
//! happens and chord-op `resolve_parent_tree_id` can't find the parent.
//! See `devlog/2026-05-19-splitblock-loro-mirror-empty.md` for the
//! full diagnosis.
//!
//! # E3 migration (2026-06-26)
//!
//! The old `E2ESut` `component_pbt!` halves + their gherkin replays
//! (`split_block_content_pbt[_full]`, `_gherkin*`) have been **deleted** —
//! they dispatched `InvBlockContentMatchesRef` over `E2ESut::SutSqlProjection`,
//! which this stack removes from `E2ESut` (E3). The split-routing regression
//! now rides on `ComposedSut<WideE2E>` via `split_block_content_composed_gherkin`
//! (the per-tick `inv-block-content-matches-ref`, in `WIDE_REQUIRED_INVARIANTS`,
//! catches mis-routing). The pure-parse outline check is SUT-agnostic and stays.
//!
//! Coverage not yet ported off the deleted `E2ESut` halves (handoff
//! `PbtComposition_SutSqlProjection_Handoff.md`, Step 1): the assert-vocabulary
//! (`widget contains` / `focus is on`) and the corrupt-id / before-startup
//! negative controls. Re-author these as born-booted composed features when
//! resumed.

#![cfg(feature = "pbt")]

/// Phase 4 expansion check: a `Scenario Outline` with N `Examples` rows must
/// expand to N independent fixtures, with `<placeholder>` substituted in the
/// Background docstring and the split step. Pure parse — no SUT.
#[test]
fn split_block_content_pbt_gherkin_outline_expands() {
    let cases = holon_integration_tests::pbt::fixtures::gherkin::parse_feature_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/split_block_content_pbt/split_outline_positions.feature"
    ))
    .expect("parse outline feature");
    assert_eq!(
        cases.len(),
        3,
        "Scenario Outline should expand to one case per Examples row"
    );
    for case in &cases {
        // Background (org file + start app) + scenario (focus + split) = 4.
        assert_eq!(case.steps.len(), 4, "{}: bg(2) + steps(2)", case.name);
    }
}

/// E3 MIGRATION PROOF (2026-06-25): the gherkin replay over `ComposedSut<WideE2E>`
/// (born-booted `full_headless` CapMap) instead of `E2ESut`. The feature is
/// re-authored onto the wide seed (no `Given org file` / `app is started`
/// ceremony — born-booted), focuses + splits `c1`; the split-routing regression
/// is caught by the per-tick composed catalog (`inv-block-content-matches-ref`,
/// in `WIDE_REQUIRED_INVARIANTS` → non-vacuous), and the assert vocabulary runs
/// via `impl FixtureAssertable for ComposedSut`.
#[test]
fn split_block_content_composed_gherkin() {
    holon_integration_tests::pbt::fixtures::run_feature_strict::<
        holon_integration_tests::pbt::composed::wide_e2e::WideE2EMachine,
        holon_integration_tests::pbt::composed::harness::ComposedSut<
            holon_integration_tests::pbt::composed::wide_e2e::WideE2E,
        >,
    >(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/composed_split_gherkin/split_routes_prefix_suffix.feature"
    ));
}
