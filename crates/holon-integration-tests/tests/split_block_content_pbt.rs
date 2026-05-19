//! `split_block_content_pbt` — narrow slice targeting the SqlOnly
//! SplitBlock content-routing divergence first surfaced by
//! `general_e2e_pbt_sql_only` (May 2026, 41 panics in one wide run).
//!
//! # Bug class
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
//! Hypotheses (ranked):
//! 1. The post-split `set_field(id, "content", content_before)` UPDATE
//!    silently no-ops or is rolled back by a downstream chord-op step.
//! 2. `block.content()` is stale at split time (SqlOnly path goes through
//!    `read_content_via_cells` → falls back to `block.content()` after the
//!    cell read returns `None`); a previous write hasn't projected yet.
//! 3. CDC matview lag swallowing the projection — downstream of (1)/(2),
//!    not the root cause.
//!
//! # Why this slice exists
//!
//! `general_e2e_pbt_sql_only` takes ~600 s and exercises 50+ transitions.
//! This slice narrows to **lifecycle + parser write + split** and runs
//! `inv-block-content-matches-ref` (per-block content equality) which
//! caught the wide-PBT divergence directly. Per-case wall expected
//! ~5-7 s; 16 cases × 1..6 steps should reproduce in under 2 minutes
//! given the ~41/per-wide-run hit rate.
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

use holon_integration_tests::declare_pbt_slice;
use holon_integration_tests::pbt::invariants::bodies::block_content_matches_ref::InvBlockContentMatchesRef;
use holon_integration_tests::pbt::transitions::{
    NavigateFocus, SplitBlock, StartApp, WriteOrgFile,
};

declare_pbt_slice! {
    test_fn: split_block_content_pbt,
    machine: SplitBlockContentMachine,
    sut_wrapper: SplitBlockContentSut,
    variant_ref: holon_integration_tests::pbt::VariantRef<holon_integration_tests::pbt::SqlOnly>,
    inner_sut: holon_integration_tests::pbt::E2ESut<holon_integration_tests::pbt::SqlOnly>,
    transitions: [
        StartApp,
        (
            WriteOrgFile,
            "skip index.org (CDC quiescence race)",
            |t: &WriteOrgFile| t.filename != "index.org",
        ),
        NavigateFocus,
        SplitBlock,
    ],
    invariants: [InvBlockContentMatchesRef],
    cases: 16,
    max_shrink_iters: 40,
    steps: 1..6,
    fixtures_test_fn: split_block_content_fixtures,
    fixtures_dir: "tests/fixtures/split_block_content_pbt",
}
