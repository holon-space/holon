//! Phase 7 — `inv-value-fn-provider-arg-variance-13` (DEFERRED).
//!
//! Inline body lives at `sut.rs:5615–5825` (value-fn provider invariants block,
//! labelled `[vfn13]`).
//!
//! # Why deferred
//!
//! The body requires:
//! - `self.reactive_engine` (private `Rc<RefCell<Option<Arc<ReactiveEngine>>>>`)
//!   to call `reactive.ui_state().set_viewport(...)`, `reactive.ensure_watching`,
//!   and `reactive.snapshot()`
//! - `crate::pbt::value_fn_invariants::{collect_providers, count_bottom_docks, rhai_mentions}`
//!   — integration-tests-internal helper module; not in `holon-pbt-core`
//! - `holon_frontend::interpret_pure` — frontend crate dependency not in pbt-core
//! - `ref_state.app_started`, `ref_state.block_state.blocks`, `ref_state.focused_block`
//!   — no Ref* cap today
//!
//! A new `SutViewModel::interpret_reactive_root(viewport)` method returning
//! `Vec<ProviderInfo>` would be needed, plus a `RefLifecycle` cap for `app_started`.
//! Wire when the ViewModel cluster cap grows the provider-inspection surface.

pub struct InvValueFnProviderArgVariance13;

impl InvValueFnProviderArgVariance13 {
    pub const ID: holon_pbt_core::invariant::InvariantId =
        holon_pbt_core::invariant::InvariantId("inv-value-fn-provider-arg-variance-13");
}

// `Invariant` impl intentionally omitted — body migration deferred.
