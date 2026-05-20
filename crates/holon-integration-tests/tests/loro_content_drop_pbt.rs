//! `loro_content_drop_pbt` — re-arm of the deterministic 9s repro for the
//! Loro content-drop bug: an `ApplyMutation` `set_field` content edit on a
//! NON-rendered block lands in Turso (`block_raw` + matview) but never reaches
//! the Loro doc, because block `set_field` is dispatched to a generic
//! SQL-authority provider instead of the Loro-authority `SqlBlockOperations`.
//!
//! The fixture lives at `tests/fixtures/general_e2e_pbt/loro-content-drop-set-field.json`
//! (3 steps: WriteOrgFile block "A" → StartApp(loro) → ApplyMutation
//! set_field content→"AK"). This slice replays it under `full()` wiring with
//! `inv-blocks-match-ref/loro` (Strict) so the divergence panics deterministically
//! in ~9s instead of the ~850s wide sweep.

#![cfg(feature = "pbt")]

use holon_integration_tests::component_pbt;
use holon_integration_tests::pbt::invariants::bodies::blocks_match_ref::InvBlocksMatchRefLoro;
use holon_integration_tests::pbt::transitions::{ApplyMutation, WriteOrgFile};

component_pbt! {
    test_fn: loro_content_drop_pbt,
    set: holon_pbt_core::ComponentSet::full_headless(),
    transitions: [
        preset lifecycle,
        WriteOrgFile,
        ApplyMutation,
    ],
    invariants: [InvBlocksMatchRefLoro],
    cases: 8,
    max_shrink_iters: 20,
    steps: 1..5,
    fixtures_dir: "tests/fixtures/general_e2e_pbt",
}
