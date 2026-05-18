//! Phase 7 — `inv-viewmodel-no-error-widgets`.
//!
//! Body derived from `sut.rs:5009–5017` (inside ReactiveEngine snapshot block,
//! sub-check 10c). Asserts no Error widget nodes exist in the headless ViewModel
//! tree produced by `interpret_pure`.
//!
//! This invariant uses `SutViewModel` because the capability's `frontend_root_is_error`
//! only covers the frontend engine root. The headless path uses the
//! `reactive_engine` snapshot which is private. The body returns `Skipped`
//! until the headless ViewModel interpretation surface is exposed.

use holon_pbt_core::capabilities::SutViewModel;
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult, RunMode};

pub struct InvViewmodelNoErrorWidgets;

impl InvViewmodelNoErrorWidgets {
    pub const ID: InvariantId = InvariantId("inv-viewmodel-no-error-widgets");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvViewmodelNoErrorWidgets
where
    S: SutViewModel,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    fn mode(&self) -> RunMode {
        RunMode::Strict
    }

    async fn check(&self, _: &R, _: &S) -> InvariantResult {
        // Blocked on Phase 7 plumbing: the headless ViewModel interpretation
        // (interpret_pure over the reactive_engine snapshot) is not yet exposed
        // via any SutViewModel capability method. The `frontend_root_is_error`
        // method covers the frontend engine only (not the headless engine).
        // Wire when SutViewModel grows a `has_error_nodes_in_headless_tree()` method.
        InvariantResult::Skipped(
            "blocked on Phase 7 plumbing: SutViewModel needs has_error_nodes_in_headless_tree() \
             to cover the headless interpret_pure path (reactive_engine is private)"
                .to_string(),
        )
    }
}
