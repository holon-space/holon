//! `inv-view-selection` — the SUT's selected view mode equals the reference's.
//! The view-selection half of the view/watches check.
//!
//! View selection is UI state, so it binds the UI component caps —
//! `SutViewModel::current_view` (SUT) and `RefRender::current_view` (ref) — not
//! a bespoke navigation cap.

use holon_pbt_core::capabilities::{RefRender, SutViewModel};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

pub struct InvViewSelection;

impl InvViewSelection {
    pub const ID: InvariantId = InvariantId("inv-view-selection");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvViewSelection
where
    R: RefRender,
    S: SutViewModel,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let sut_view = sut.current_view().await;
        let ref_view = ref_.current_view();
        if sut_view == ref_view {
            return InvariantResult::Ok;
        }
        InvariantResult::Fail(format!(
            "[inv-view-selection] selected view diverged: SUT={sut_view:?} ref={ref_view:?}"
        ))
    }
}
