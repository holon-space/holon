//! `extended_gen_pbt` — Phase 3 generator-coverage expansion slice,
//! env-gated behind `HOLON_PBT_EXTENDED_GEN=1`.
//!
//! # Why this slice exists
//!
//! The default generators are deliberately conservative (ASCII-only,
//! non-empty, flat files) — that hides whole bug classes (byte-offset vs
//! keystroke-count conversions, empty-content rendering, org-special
//! escaping). Each Phase 3 axis adds an extended arm to the shared
//! generators, gated on `HOLON_PBT_EXTENDED_GEN` so the blessed default
//! gates stay green while this slice surfaces and triages the new failures
//! one axis at a time. An axis is promoted into the defaults (the env gate
//! removed for its arm) once this slice is green for it.
//!
//! Active axes:
//! 1. **Content** (`generators.rs::extended_content_arm`): 2/3/4-byte
//!    codepoints, empty strings, whitespace-only, org-special prefixes —
//!    wired into `content_strategy`, `edit_content_strategy`,
//!    `typing_text_strategy` (TypeChars), `peer_content_strategy`
//!    (PeerEdit), `bulk_content_strategy` (BulkExternalAdd), and the
//!    seeded-file headline generator.
//!
//! # Running
//!
//! ```text
//!   HOLON_PBT_EXTENDED_GEN=1 cargo nextest run -p holon-integration-tests \
//!     --test extended_gen_pbt
//! ```
//!
//! Without the env var the test discloses the skip and runs 0 cases — the
//! same full-coverage machine as `general_e2e_pbt` would otherwise burn
//! minutes re-running default generators here.

#![cfg(feature = "pbt")]

use holon_integration_tests::component_pbt;
use holon_integration_tests::pbt::standard_pbt_config;
use holon_pbt_core::ComponentSet;

/// `standard_pbt_config`, with `cases` forced to 0 (a disclosed no-op) when
/// `HOLON_PBT_EXTENDED_GEN` is not set. The extended slice keeps its own
/// regressions file — its failures replay only under the extended arms, so
/// sharing `general_e2e_pbt`'s file would poison the default gate's replays.
fn extended_pbt_config() -> proptest::test_runner::Config {
    let mut config = standard_pbt_config("extended_gen_pbt");
    if std::env::var("HOLON_PBT_EXTENDED_GEN").as_deref() != Ok("1") {
        eprintln!(
            "[extended_gen_pbt] HOLON_PBT_EXTENDED_GEN not set — skipping (0 cases). \
             Set HOLON_PBT_EXTENDED_GEN=1 to run the extended-generator sweep."
        );
        config.cases = 0;
    }
    config
}

component_pbt! {
    test_fn: extended_gen_pbt_sql_only,
    set: ComponentSet::sql_only(),
    proptest_config: extended_pbt_config(),
    steps: 3..20,
}

// Full/Loro twin — unblocked 2026-06-10: the SplitBlock→DeleteBackward
// divergence was the join_block SQL-direct delete (fixed via
// delete_block_via_cells; see memory full_deletebackward_join_fix_2026-06-10).
component_pbt! {
    test_fn: extended_gen_pbt_full,
    set: ComponentSet::full_headless(),
    proptest_config: extended_pbt_config(),
    steps: 3..20,
}
