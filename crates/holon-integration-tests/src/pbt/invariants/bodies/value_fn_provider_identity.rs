//! Phase 7 — `inv-value-fn-provider-identity` (DEFERRED).
//!
//! Inline body lives at `sut.rs:5834–5887` (the intermediate ViewModel
//! emissions check, labelled `[inv-value-fn-provider-identity]`).
//!
//! # Why deferred
//!
//! The body requires:
//! - `self.vm_emissions` (private `Arc<Mutex<Vec<holon_frontend::ViewModel>>>`) —
//!   collects all intermediate ViewModel emissions across the transition.
//!   `SutViewModel::drain_vm_emissions` is the planned cap method but is currently
//!   `unimplemented!()` on `E2ESut` (sut_capabilities.rs:298).
//! - `ref_state.block_state.blocks` — to look up expected `task_state` per block
//! - `holon_frontend::view_model::ViewKind::StateToggle` — frontend type not in pbt-core
//!
//! Wire when `SutViewModel::drain_vm_emissions` is implemented on `E2ESut` and a
//! new `SutViewModel::vm_emission_toggle_values()` returns `Vec<(block_id, value)>`
//! abstracting away the `ViewModel` type dependency.

pub struct InvValueFnProviderIdentity;

impl InvValueFnProviderIdentity {
    pub const ID: holon_pbt_core::invariant::InvariantId =
        holon_pbt_core::invariant::InvariantId("inv-value-fn-provider-identity");
}

// `Invariant` impl intentionally omitted — body migration deferred.
