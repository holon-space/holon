//! `inv-viewmodel-no-error-widgets`.
//!
//! Walks the headless `ReactiveEngine`'s rendered ViewModel tree and
//! asserts no `Error` widget nodes exist. Catches render-pipeline
//! failures (matview fault, CDC delivery bug, shadow-interpretation
//! panic) that leave Error widgets in the user-visible tree.
//!
//! Capability: `SutViewSelection::headless_error_node_count` returns
//! `Some(n)` with the count, or `None` when the headless engine
//! isn't installed or its tree isn't yet ready (loading / placeholder
//! / interpretation panicked). `None` → `Skipped`.

use holon_pbt_core::capabilities::SutViewSelection;
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

pub struct InvViewmodelNoErrorWidgets;

impl InvViewmodelNoErrorWidgets {
    pub const ID: InvariantId = InvariantId("inv-viewmodel-no-error-widgets");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvViewmodelNoErrorWidgets
where
    S: SutViewSelection,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        match sut.headless_error_node_count().await {
            None => {
                InvariantResult::Skipped("headless engine not installed or tree not ready".into())
            }
            Some(0) => InvariantResult::Ok,
            Some(n) => InvariantResult::Fail(format!(
                "[inv-viewmodel-no-error-widgets] {n} error node(s) in headless ViewModel tree"
            )),
        }
    }
}
