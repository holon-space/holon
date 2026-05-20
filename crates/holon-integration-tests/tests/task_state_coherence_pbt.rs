//! `task_state_coherence_pbt` — Full-mode slice exercising
//! `inv-task-state-storage-coherence` end-to-end.
//!
//! Phase 1 of the PBT reuse plan wired `SutLoroTaskState::loro_task_state_of`
//! on `E2ESut` (it was previously `unimplemented!`). This slice is the live
//! proof: it creates task blocks via org writes, toggles their state, and
//! after every step asserts the Loro projection of `task_state` agrees with
//! the SQL `block_raw.properties` projection.
//!
//! Full-only by construction: in SqlOnly mode there is no Loro document, so
//! `loro_task_state_of` has nothing to project and the coherence check is
//! meaningless.

#![cfg(feature = "pbt")]

use holon_integration_tests::component_pbt;
use holon_integration_tests::pbt::invariants::bodies::task_state_storage_coherence::InvTaskStateStorageCoherence;
use holon_integration_tests::pbt::transitions::{ToggleState, WriteOrgFile};

component_pbt! {
    test_fn: task_state_coherence_pbt_full,
    set: holon_pbt_core::ComponentSet::full_headless(),
    transitions: [
        preset lifecycle,
        WriteOrgFile,
        ToggleState,
    ],
    invariants: [
        InvTaskStateStorageCoherence(::std::marker::PhantomData::<holon_integration_tests::pbt::ReferenceState>),
    ],
    cases: 16,
    max_shrink_iters: 20,
    steps: 1..6,
}
