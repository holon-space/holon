//! Phase 7 — first migrated invariant: `inv-loro-no-errors`.
//!
//! Proof point for the `Invariant<R, S>` trait + capability-bound
//! evaluation. The body comes verbatim from `sut.rs:4099-4113`; the
//! difference is now any slice whose SUT implements `SutLoroLog`
//! gets this invariant for free, with no inline closure in
//! `check_invariants_async`.
//!
//! 1-subsystem invariant — touches only the Loro log; not gated by
//! the CDC-lag classifier.

use holon_pbt_core::capabilities::SutLoroLog;
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult, RunMode};

pub struct InvLoroNoErrors;

impl InvLoroNoErrors {
    pub const ID: InvariantId = InvariantId("inv-loro-no-errors");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvLoroNoErrors
where
    S: SutLoroLog,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    fn mode(&self) -> RunMode {
        RunMode::Strict
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        if sut.loro_had_errors().await {
            InvariantResult::Fail(
                "[inv-loro-no-errors] LoroSyncController logged error(s). \
                 Search captured logs for `[LoroSyncController] Failed to apply` \
                 to find which event(s) the SQL→Loro mirror dropped (e.g. \
                 `Cannot resolve parent URI to TreeID: block:UUID` for \
                 outdent/indent/split where the new parent isn't yet a \
                 TreeID in the Loro tree)."
                    .to_string(),
            )
        } else {
            InvariantResult::Ok
        }
    }
}
