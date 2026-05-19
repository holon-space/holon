//! `cdc_delivery_pbt` — Phase B follow-up slice (PbtSlicing.md candidate #2).
//!
//! # Why this slice exists
//!
//! Targets the matview→CDC→watch delivery chain. MEMORY.md lists four
//! historically-painful Turso IVM bug classes — `json_group_array` multiset
//! negative, MatchCounterOperator Uninitialized, `focus_roots` LEFT-JOIN drop,
//! IVM matview-cursor first-open empty — every one of them surfaces in the
//! same mechanism: a watch subscriber reads stale or extra rows from a
//! materialised view after a write that should have propagated through the
//! CDC pipeline.
//!
//! The wide PBT catches these but at minutes/case; `storage_consistency_pbt`
//! catches them at ~7.7 s/case but exercises peer-merge / org-file / chord-op
//! transitions that aren't the dominant suspects for matview drift. This
//! slice narrows to **storage mutations + watch lifecycle**, concentrating
//! the proptest shrinker on the transitions most likely to trip a CDC bug.
//!
//! See `crates/holon-integration-tests/src/pbt/slice.rs` for the
//! `declare_pbt_slice!` macro that drives the boilerplate. The
//! `WriteOrgFile` filter skips `index.org` (CDC quiescence race during
//! StartApp — known race in the E2E infrastructure, see
//! `storage_consistency_pbt.rs` for the same workaround).
//!
//! # Invariants
//!
//! - `InvLoroNoErrors` — required on any Loro-touching slice; matview-CDC
//!   bugs often present first as a `[LoroSyncController] Failed to apply`
//!   log line because the SQL→Loro mirror drops events the matview lost.
//! - `InvBlockTagsReferencesExist` — orphan `block_tags` rows are a direct
//!   matview-drift signature (the junction table outliving a `block_raw`
//!   delete is the exact shape that produces multiset-negative panics on
//!   the next aggregation).
//!
//! # Cost
//!
//! Targets ~30 s wall for 16 cases × 1..10 steps. Reuses `E2ESut<SqlOnly>`
//! init cost (~7-8 s/case), which is the dominant overhead.

use std::marker::PhantomData;

use holon_integration_tests::declare_pbt_slice;
use holon_integration_tests::pbt::invariants::bodies::block_tags_references_exist::InvBlockTagsReferencesExist;
use holon_integration_tests::pbt::invariants::bodies::loro_no_errors::InvLoroNoErrors;
use holon_integration_tests::pbt::transitions::{
    BulkExternalAdd, JoinBlock, RemoveWatch, SetupWatch, SplitBlock, StartApp, TypeChars,
    WriteOrgFile,
};

declare_pbt_slice! {
    test_fn: cdc_delivery_pbt,
    machine: CdcDeliveryMachine,
    sut_wrapper: CdcDeliverySut,
    variant_ref: holon_integration_tests::pbt::VariantRef<holon_integration_tests::pbt::SqlOnly>,
    inner_sut: holon_integration_tests::pbt::E2ESut<holon_integration_tests::pbt::SqlOnly>,
    transitions: [
        StartApp,
        (
            WriteOrgFile,
            "skip index.org (CDC quiescence race)",
            |t: &WriteOrgFile| t.filename != "index.org",
        ),
        BulkExternalAdd,
        SplitBlock,
        JoinBlock,
        TypeChars,
        SetupWatch,
        RemoveWatch,
    ],
    invariants: [
        InvLoroNoErrors,
        InvBlockTagsReferencesExist(PhantomData::<holon_integration_tests::pbt::ReferenceState>),
    ],
    cases: 16,
    max_shrink_iters: 20,
    steps: 1..10,
}
