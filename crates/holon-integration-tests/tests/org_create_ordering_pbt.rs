//! `org_create_ordering_pbt` — narrow Full-mode slice targeting the
//! org-ingest → Loro → SQL **sibling-ordering** bug class.
//!
//! # Bug class
//!
//! Org-ingested blocks that are created but never repositioned never get
//! their Loro fractional index projected to SQL `sort_key` (it stays the
//! default `"A0"`), so they mis-sort against moved siblings carrying a real
//! fi (`817F…` < `"A0"` lexically). Symptom in the wide PBT: `assert_block_order`
//! under `block:ref-doc-N` — `bulk-*` siblings render ahead of an existing
//! first child. Root: the projection-totality gap (complete-authority create
//! + single-writer projection is the fix).
//!
//! # Why this slice exists
//!
//! `general_e2e_pbt` (Full) takes minutes and exercises 50+ transitions; its
//! deep runs also trip an unrelated intermittent race family that muddies the
//! signal. This slice narrows to **lifecycle + org write + bulk add** and
//! runs `inv-live-children-match-ref` (per-parent sibling-order equality:
//! SQL `ORDER BY sort_key` vs the ref model's document order) — the check
//! that catches the divergence directly. It is the fast green gate for the
//! authority/ordering refactor.
//!
//! SqlOnly is deliberately NOT a variant here: in SqlOnly mode `sort_key` is
//! SQL-authoritative (`gen_key_between`), there is no Loro→SQL projection, so
//! the bug cannot occur — only `Full` exercises the failing path.

#![cfg(feature = "pbt")]

use holon_integration_tests::component_pbt;
use holon_integration_tests::pbt::invariants::bodies::live_children_match_ref::InvLiveChildrenMatchRef;
use holon_integration_tests::pbt::transitions::{BulkExternalAdd, WriteOrgFile};

component_pbt! {
    test_fn: org_create_ordering_pbt_full,
    set: holon_pbt_core::ComponentSet::full_headless(),
    transitions: [
        preset lifecycle,
        WriteOrgFile,
        BulkExternalAdd,
    ],
    invariants: [InvLiveChildrenMatchRef],
    cases: 16,
    max_shrink_iters: 20,
    steps: 1..6,
}
