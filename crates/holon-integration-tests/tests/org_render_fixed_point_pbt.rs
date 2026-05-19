//! `org_render_fixed_point_pbt` — narrow slice targeting the
//! `inv-org-render-fixed-point` flake first surfaced by `general_e2e_pbt`
//! (see `devlog/2026-05-19-phase-c-validation-diagnosis.md`).
//!
//! # Bug class
//!
//! After `BulkExternalAdd` writes an org file with a `#+TODO:` header, the
//! doc row's `block_raw.properties.todo_keywords` ends up missing — so the
//! next render-from-SQL drops the `#+TODO:` line, the renderer's output
//! differs from disk, and `re_render_all_tracked` would rewrite the file
//! (echo-suppression loop risk).
//!
//! `general_e2e_pbt` reproduces this seed in ~250s of wall-clock per
//! variant. This slice narrows the transition surface to just the path
//! that triggers the bug — `WriteOrgFile` (regular files) + `StartApp` +
//! `BulkExternalAdd` — concentrating the proptest shrinker on the
//! offending sequence.
//!
//! # Why these transitions only
//!
//! - `StartApp` is the lifecycle bootstrap; required for any SQL writes.
//! - `WriteOrgFile` (non-index.org) seeds the doc via the org parser path.
//! - `BulkExternalAdd` is the suspect: it writes the file with a
//!   keyword-set-bearing header and waits for block-count sync but not
//!   for doc-metadata sync.
//!
//! Deliberately omitted: peer/Loro transitions (no Loro here — `SqlOnly`),
//! navigation/UI (no frontend), content mutations (the bug surfaces at
//! the doc-level metadata seam, not via chord ops).
//!
//! # Cost target
//!
//! ~60s wall for 16 cases × 1..6 steps. The shrunk seed in the wide-PBT
//! failure used 4 transitions, so this is generous.

#![cfg(feature = "pbt")]

use holon_integration_tests::declare_pbt_slice;
use holon_integration_tests::pbt::invariants::bodies::org_render_fixed_point::InvOrgRenderFixedPoint;
use holon_integration_tests::pbt::transitions::{BulkExternalAdd, NavigateFocus, SplitBlock};

declare_pbt_slice! {
    test_fn: org_render_fixed_point_pbt,
    variant_ref: holon_integration_tests::pbt::VariantRef<holon_integration_tests::pbt::SqlOnly>,
    inner_sut: holon_integration_tests::pbt::E2ESut<holon_integration_tests::pbt::SqlOnly>,
    transitions: [
        preset lifecycle,
        BulkExternalAdd,
        NavigateFocus,
        SplitBlock,
    ],
    invariants: [InvOrgRenderFixedPoint],
    cases: 16,
    max_shrink_iters: 20,
    steps: 1..8,
    fixtures_dir: "tests/fixtures/org_render_fixed_point_pbt",
}
