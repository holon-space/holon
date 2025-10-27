//! `inv-no-errors` — app-runtime error log is empty.
//!
//! The general "did the app error during this run" guard, distinct from the
//! component-specific error invariants (`inv-loro-no-errors`,
//! `inv-viewmodel-no-error-widgets`, `inv-frontend-no-error-widgets`,
//! `inv-frontend-root-not-error`). Today its source is the Flutter/event
//! publish errors logged during the initial document sync — the
//! `check_inv_no_startup_errors` guard, generalised into a registered
//! invariant via the [`SutErrorLog`] component cap.

use holon_pbt_core::capabilities::SutErrorLog;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvNoErrors;

impl InvNoErrors {
    pub const ID: InvariantId = InvariantId("inv-no-errors");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvNoErrors
where
    S: SutErrorLog,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        let count = sut.app_error_count().await;
        if count == 0 {
            return InvariantResult::Ok;
        }
        let docs = sut.app_error_context().await;
        InvariantResult::Fail(format!(
            "[inv-no-errors] {count} app publish error(s) during startup. Indicates a DDL/sync \
             race when {} document(s) were synced.\n  documents: {docs:?}",
            docs.len(),
        ))
    }
}
