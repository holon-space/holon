//! `inv-frontend-root-not-error`.
//!
//! Checks that the frontend's root ViewModel node is not the Error variant.
//!
//! 1-subsystem invariant — touches only ViewModel.

use holon_pbt_core::capabilities::SutViewModel;
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

pub struct InvFrontendRootNotError;

impl InvFrontendRootNotError {
    pub const ID: InvariantId = InvariantId("inv-frontend-root-not-error");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvFrontendRootNotError
where
    S: SutViewModel,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        if sut.frontend_root_is_error().await {
            InvariantResult::Fail(
                "[inv-frontend-root-not-error] Frontend root widget is Error. \
                 Search captured logs for `error_message` in the root ViewModel \
                 snapshot to find which render expression produced the error node."
                    .to_string(),
            )
        } else {
            InvariantResult::Ok
        }
    }
}
