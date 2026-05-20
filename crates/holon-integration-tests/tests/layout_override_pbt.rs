//! `layout_override_pbt` — CI coverage for the `index.org` layout-override
//! generator arms (axis 4 leftover).
//!
//! The default gates run with layout overrides OFF (`write_org_file.rs`
//! reads `HOLON_PBT_LAYOUT_OVERRIDE` per generator call): a vanilla seed
//! layout keeps the edit/split/cursor transitions reachable. That leaves
//! the four `index.org` override variants (prql/gql/sql layouts + the
//! profile file arm) with zero CI coverage. This slice flips the env on
//! for its own process so those arms generate, while the blessed default
//! gates stay untouched.
//!
//! Like `extended_gen_pbt`, the slice is env-gated (`HOLON_PBT_LAYOUT_OVERRIDE=1`
//! — the same var that enables the generator arm) while its findings are
//! triaged; it graduates to always-on CI once green. It keeps its own
//! regressions file: its failures replay only with the override arms active,
//! so sharing `general_e2e_pbt`'s file would poison the default gate's replays.
//!
//! Triage history: the original decompiled-rows + ghost-row + Loro-swap
//! findings are ALL FIXED (fixed-ID `:ID: root-layout` contract + 4 prod bugs,
//! `ReactiveRenderedRows` regeneration snapshot, `EntityUri::from_raw` `::`-id
//! scheme mis-parse). 4-case sweeps green on BOTH twins (2026-06-11).
//!
//! KNOWN RED (2026-06-11, PRE-EXISTING — replays identically with the from_raw
//! fix reverted): 8-case full twin hits the link-mark family — UI-origin
//! `ApplyMutation` Update with `[[label]]` content on `block:root-layout`
//! keeps the raw `[[…]]` in the SUT while the ref expects the extracted
//! label ("Post-mutation spot-check timeout … expected "xO75 mL74J6", got
//! "[[xO75 mL74J6]]""). Same family as the split_full `[[ x]]` divergence
//! (/tmp/split_full_linkmark_divergence.captured.json). Capture:
//! /tmp/layout_full_8case_finding.captured.json (3 transitions, signature
//! "Post-mutation spot-check timeout", ceiling full_headless). Un-gate once
//! the link-mark extraction path is reconciled.

#![cfg(feature = "pbt")]

use holon_integration_tests::component_pbt;
use holon_integration_tests::pbt::standard_pbt_config;
use holon_pbt_core::ComponentSet;

/// `standard_pbt_config`, with `cases` forced to 0 (a disclosed no-op) when
/// `HOLON_PBT_LAYOUT_OVERRIDE` is not set. The env does double duty: it
/// un-skips this slice AND enables the index-override arms in
/// `write_org_file.rs` (read per `weighted_generator` call).
fn layout_override_pbt_config() -> proptest::test_runner::Config {
    let mut config = standard_pbt_config("layout_override_pbt");
    if std::env::var("HOLON_PBT_LAYOUT_OVERRIDE").as_deref() != Ok("1") {
        eprintln!(
            "[layout_override_pbt] HOLON_PBT_LAYOUT_OVERRIDE not set — skipping (0 cases). \
             Set HOLON_PBT_LAYOUT_OVERRIDE=1 to run the layout-override sweep."
        );
        config.cases = 0;
    }
    config
}

component_pbt! {
    test_fn: layout_override_pbt_sql_only,
    set: ComponentSet::sql_only(),
    proptest_config: layout_override_pbt_config(),
    steps: 3..20,
}

component_pbt! {
    test_fn: layout_override_pbt_full,
    set: ComponentSet::full_headless(),
    proptest_config: layout_override_pbt_config(),
    steps: 3..20,
}
