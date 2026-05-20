//! a1 (ADR 0004 Phase 9, part (a)): the first **Loro-wired** PBT slice.
//!
//! `Wiring::loro_backend()` = `{Loro}` (no Turso). The slice macro's
//! `init_test` maps that manifest to `StorageSelector::LoroMemory` (via
//! `storage_selector_for_wiring`), so this slice drives the **no-Turso** SUT:
//! `start_app` builds a Turso-free DI container (no `BackendEngine`), reads
//! structural blocks through `BlockQuerySource`, and routes mutations through
//! the Loro-native `OperationEngine`.
//!
//! This is the headless companion to the gpui layout slice: it proves the
//! wiring-parameterized phased runner can drive Loro churn end-to-end and that
//! the Loro-side invariants hold. Turso-only transitions and invariants are
//! skipped by the ADR-0007 RequiredWiring gating (the manifest supplies no
//! Turso adapter), so the registry reduces to the subset `{Loro}` can satisfy
//! (e.g. `inv-loro-no-errors`, `inv-loro-children-match-ref`,
//! `inv-blocks-match-ref/loro`) plus the wiring-agnostic checks.

use holon_integration_tests::component_pbt;
use holon_integration_tests::pbt::standard_pbt_config;
use holon_pbt_core::ComponentSet;

// ADR 0009 Goal 1: a blessed slice expressed as a one-line `ComponentSet`.
// `ComponentSet::loro_vm_fast().wiring == Wiring::loro_backend()`, so this is
// behaviour-identical to the prior `declare_pbt_slice! { wiring: … }` form.
component_pbt! {
    test_fn: loro_backend_pbt,
    set: ComponentSet::loro_vm_fast(),
    proptest_config: standard_pbt_config("loro_backend_pbt"),
    steps: 3..12,
}
