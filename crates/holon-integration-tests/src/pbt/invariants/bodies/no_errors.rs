//! `inv-no-errors` — app-runtime error log is empty.
//!
//! @pbt oracle internal-consistency
//! @pbt covers app-error-log-empty — nothing recorded on the session's
//!   publish-error tracker since boot (the app-level counter, distinct from
//!   widget-tree errors)
//! @pbt slips-if-removed any op dispatch failure during the run ships
//!   silently; the UI may look fine while a write never landed and nothing
//!   else asserts on it
//!
//! The general "did the app error during this run" guard, distinct from the
//! component-specific error invariants (`inv-loro-no-errors`,
//! `inv-viewmodel-no-error-widgets`, `inv-frontend-no-error-widgets`,
//! `inv-frontend-root-not-error`). Its source is the session's
//! `PublishErrorTracker` via the [`SutErrorLog`] component cap. Every one of
//! that tracker's `record_error` call sites is a mid-run op failure
//! (`disclose_op_failure`, the fire-and-forget op dispatch, the
//! `preferences.set` persist), so a non-zero count never means boot. The
//! counter holds no message, so the failing ERROR text comes from
//! `inv-no-observed-errors`.

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
            "[inv-no-errors] {count} app error(s) on the session publish-error tracker: an op \
             dispatch that failed during the run. The counter carries no message — read the \
             ERROR text from `inv-no-observed-errors`.\n  {} document(s) synced at boot: \
             {docs:?}",
            docs.len(),
        ))
    }
}
