//! `split_block_content_pbt` — narrow slice targeting two SplitBlock
//! content-routing bugs that share the same minimal recipe (lifecycle +
//! parser-seeded doc + split).
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
//! full diagnosis. Same minimal shape as Bug A — WriteOrgFile +
//! SplitBlock — but the failure mode is a chord-op error rather than
//! a content divergence, which surfaces through the existing
//! `inv-block-content-matches-ref` plus error invariants on the SUT.
//!
//! # Why this slice exists
//!
//! `general_e2e_pbt` takes ~10 min and exercises 50+ transitions. This
//! slice narrows to **lifecycle + parser write + split** and runs
//! `inv-block-content-matches-ref` (per-block content equality) which
//! caught the wide-PBT divergence directly. Per-case wall expected
//! ~5-7 s; 16 cases × 1..6 steps should reproduce in under 2 minutes
//! per variant.
//!
//! # Transitions
//!
//! - `StartApp` — required.
//! - `WriteOrgFile` (non-index.org) — seeds the doc via the parser path
//!   so split targets are parser-created blocks, matching the failing
//!   wide-PBT shape (vs `BulkExternalAdd`'s loro-tree blocks).
//! - `NavigateFocus` — needed to put cursor in a region before split.
//! - `SplitBlock` — the suspect.
//!
//! Deliberately omitted: peer/Loro transitions (no Loro here — `SqlOnly`),
//! BulkExternalAdd (different write path), TypeChars/DeleteBackward
//! (would introduce in-flight cell writes confounding the diagnosis).
//!
//! # Capture-on-panic
//!
//! `declare_pbt_slice!` installs a thread-local capture buffer; the first
//! panic dumps the failing transition sequence to
//! `tests/fixtures/split_block_content_pbt/captured-*.json` for replay.

#![cfg(feature = "pbt")]

use holon_integration_tests::component_pbt;
use holon_integration_tests::pbt::invariants::bodies::block_content_matches_ref::InvBlockContentMatchesRef;
use holon_integration_tests::pbt::transitions::{NavigateFocus, SplitBlock};

component_pbt! {
    test_fn: split_block_content_pbt,
    set: holon_pbt_core::ComponentSet::sql_only(),
    transitions: [
        preset lifecycle,
        preset org_writes,
        NavigateFocus,
        SplitBlock,
    ],
    invariants: [InvBlockContentMatchesRef],
    cases: 16,
    max_shrink_iters: 40,
    steps: 1..6,
    fixtures_dir: "tests/fixtures/split_block_content_pbt",
}

component_pbt! {
    test_fn: split_block_content_pbt_full,
    set: holon_pbt_core::ComponentSet::full_headless(),
    transitions: [
        preset lifecycle,
        preset org_writes,
        NavigateFocus,
        SplitBlock,
    ],
    invariants: [InvBlockContentMatchesRef],
    cases: 16,
    max_shrink_iters: 40,
    steps: 1..6,
    fixtures_dir: "tests/fixtures/split_block_content_pbt_full",
}

/// Phase 1 proof-point: replay a hand-authored Gherkin `.feature` through the
/// same SUT machine + `InvBlockContentMatchesRef` as the JSON fixtures, but
/// with strict semantics (a failed precondition is a hard panic, never a
/// silent skip). Drives the `SqlOnly` slice.
#[test]
fn split_block_content_pbt_gherkin() {
    holon_integration_tests::pbt::fixtures::run_feature_strict::<
        SplitBlockContentPbtMachine,
        SplitBlockContentPbtSut,
    >(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/split_block_content_pbt/split_routes_prefix_suffix.feature"
    ));
}

/// Phase 3 assert vocabulary: replay a feature with `Then` widget-contains +
/// focus-on assertions. Surfaces real harness state — fails loud if the SUT
/// can't render / track focus.
#[test]
fn split_block_content_pbt_gherkin_asserts() {
    holon_integration_tests::pbt::fixtures::run_feature_strict::<
        SplitBlockContentPbtMachine,
        SplitBlockContentPbtSut,
    >(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/_gherkin_assert/widget_and_focus.feature"
    ));
}

/// VT1: a block created by `SplitBlock` is addressable as `block::split-N`.
/// Asserts focus + rendered content on the new split block (and the trimmed
/// original), exercising synthetic-id resolution and `within N seconds`.
#[test]
fn split_block_content_pbt_gherkin_split_addressing() {
    holon_integration_tests::pbt::fixtures::run_feature_strict::<
        SplitBlockContentPbtMachine,
        SplitBlockContentPbtSut,
    >(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/_gherkin_assert/split_then_address_new_block.feature"
    ));
}

/// Negative control: a `Then` assertion before the app is started is vacuous
/// (no rendered state) and must HARD PANIC, never silently pass.
#[test]
#[should_panic(expected = "vacuous")]
fn split_block_content_pbt_gherkin_assert_before_startup_panics() {
    holon_integration_tests::pbt::fixtures::run_feature_strict::<
        SplitBlockContentPbtMachine,
        SplitBlockContentPbtSut,
    >(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/_gherkin_negative/then_before_startup.feature"
    ));
}

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

/// Negative control: a `.feature` whose split target references a block whose
/// `:ID:` was corrupted must HARD PANIC under strict replay (precondition
/// failure), never silently skip the step and report green.
#[test]
#[should_panic(expected = "preconditions FAILED")]
fn split_block_content_pbt_gherkin_corrupt_id_panics() {
    holon_integration_tests::pbt::fixtures::run_feature_strict::<
        SplitBlockContentPbtMachine,
        SplitBlockContentPbtSut,
    >(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/_gherkin_negative/split_corrupt_id.feature"
    ));
}

/// E3 MIGRATION PROOF (2026-06-25): the SAME gherkin replay over `ComposedSut<WideE2E>`
/// (born-booted `full_headless` CapMap) instead of `E2ESut`. The feature is re-authored
/// onto the wide seed (no `Given org file` / `app is started` ceremony — born-booted),
/// focuses + splits `c1`; the split-routing regression is caught by the per-tick composed
/// catalog (`inv-block-content-matches-ref`, in `WIDE_REQUIRED_INVARIANTS` → non-vacuous),
/// and the assert vocabulary runs via `impl FixtureAssertable for ComposedSut`. Once the
/// remaining features are ported (handoff), the `E2ESut` `component_pbt!` halves + the old
/// `.feature` corpus above are deleted and `SutSqlProjection` comes off `E2ESut`.
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
