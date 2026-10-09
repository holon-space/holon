//! `inv-crash-history-matches-ref` — every crash record on disk is readable in
//! full from inside Holon, and reading it changes no file.
//!
//! @pbt oracle model-equivalence — the crash history lists exactly the records
//!   the model knows were written, newest first, each with its full message,
//!   location and where it is kept, and no other entry
//! @pbt covers crash-history-view — the records a prior crash or a task panic
//!   leave, before and after a restart moves this run's record among the
//!   unshown ones (D-crash-history.a)
//! @pbt slips-if-removed the history drops a record, shortens its message, or
//!   reading it moves or rewrites a record file

use holon_pbt_core::capabilities::RefCrashHistory;
use holon_pbt_core::capabilities::SutCrashHistory;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvCrashHistoryMatchesRef;

impl InvCrashHistoryMatchesRef {
    pub const ID: InvariantId = InvariantId("inv-crash-history-matches-ref");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvCrashHistoryMatchesRef
where
    R: RefCrashHistory,
    S: SutCrashHistory,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, reference: &R, sut: &S) -> InvariantResult {
        let expected = reference.expected_crash_history();
        let read = sut.crash_history_now().await;
        if !read.changed_by_reading.is_empty() {
            return InvariantResult::Fail(format!(
                "reading the crash history changed record files: {:?}",
                read.changed_by_reading
            ));
        }
        if read.records != expected || !read.other_entries.is_empty() {
            return InvariantResult::Fail(format!(
                "the crash history shows {} record(s) where the model has {}.\nmodel (newest \
                 first): {expected:#?}\nhistory: {:#?}\nother entries: {:?}",
                read.records.len(),
                expected.len(),
                read.records,
                read.other_entries
            ));
        }
        InvariantResult::Ok
    }
}
