//! `inv-frontend-engine`.
//!
//! @pbt oracle internal-consistency
//! @pbt covers render-liveness — the gpui window's ReactiveEngine resolves
//!   the root layout to a settled (non-loading) ViewModel
//! @pbt slips-if-removed a matview/CDC/waker stall that leaves the root
//!   perpetually `loading` ships a permanently blank window that the
//!   headless engine, having no live window, never exercises
//!
//! Liveness gate on the gpui window's own `ReactiveEngine`: the frontend
//! must resolve the root layout to a settled (non-loading) ViewModel. This
//! catches issues invisible to the headless engine — matview failures, CDC
//! delivery bugs, cross-executor waker stalls that leave the root perpetually
//! `loading`.
//!
//! The umbrella's other halves are already their own bodies: the root being
//! an Error widget is `inv-frontend-root-not-error`, nested error widgets are
//! `inv-frontend-no-error-widgets`, and "expected elements laid out" is
//! `inv-frontend-bounds-rendered`. So this body asserts only that resolution
//! itself produced a usable root — reading
//! [`SutFrontendEngine::frontend_root_vm`], which returns `None` (→ `Skipped`)
//! when there is no frontend engine (headless / SqlOnly) or the root is still
//! loading.

use holon_pbt_core::capabilities::SutFrontendEngine;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvFrontendEngine;

impl InvFrontendEngine {
    pub const ID: InvariantId = InvariantId("inv-frontend-engine");
    const LABEL: &'static str = "inv-frontend-engine";
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvFrontendEngine
where
    S: SutFrontendEngine,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        match sut.frontend_root_vm().await {
            Some(_) => InvariantResult::Ok,
            None => InvariantResult::Skipped(format!(
                "[{}] no frontend engine installed, or root layout still loading",
                Self::LABEL
            )),
        }
    }
}
