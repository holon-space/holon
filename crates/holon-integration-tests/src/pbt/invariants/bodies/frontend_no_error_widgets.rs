//! `inv-frontend-no-error-widgets`.
//!
//! @pbt oracle internal-consistency
//! @pbt covers no-error-widgets — no Error widget in the laid-out
//!   BoundsRegistry (authoritative) or, absent geometry, the ViewModel tree
//! @pbt slips-if-removed a render expression that evaluates to an error node
//!   (bad query, missing field, interp panic) renders a red error card in
//!   the live window where the user would see it but no assertion fires
//!
//! Asserts no Error widgets exist in the rendered frontend ViewModel tree.
//!
//! 2-subsystem invariant — ViewModel + FrontendBounds. The BoundsRegistry
//! path is the more authoritative check; `SutLayout::any_error_widget` falls
//! back to the ViewModel tree when no geometry provider is installed.

use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::capabilities::SutViewSelection;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvFrontendNoErrorWidgets;

impl InvFrontendNoErrorWidgets {
    pub const ID: InvariantId = InvariantId("inv-frontend-no-error-widgets");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvFrontendNoErrorWidgets
where
    S: SutViewSelection + SutLayout,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        if sut.any_error_widget().await {
            InvariantResult::Fail(
                "[inv-frontend-no-error-widgets] Frontend ViewModel or BoundsRegistry contains \
                 Error widget(s). Search captured logs for `error_message` nodes in the ViewModel \
                 tree to identify the broken render expression."
                    .to_string(),
            )
        } else {
            InvariantResult::Ok
        }
    }
}
