//! Taking back a multi-op batch the block write authority partly applied.
//!
//! A caller that dispatches N operations one by one holds no transaction over
//! them: op K failing leaves ops `0..K` committed. Where the authority is a
//! CRDT it can undo exactly the batch's own writes, and this is the seam that
//! offers it — `None` from
//! [`OperationProvider::batch_rollback`](crate::OperationProvider::batch_rollback)
//! means this session's authority cannot, and the caller owes its user a
//! partial-apply disclosure instead.
//!
//! A write belongs to the batch when it runs inside [`BatchId::scope`] on the
//! batch's own task. Writes from any other task, thread or peer are not the
//! batch's, and the rollback keeps them.

use std::future::Future;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use async_trait::async_trait;

/// The identity a batch's writes carry, unique per batch in this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BatchId(u64);

tokio::task_local! {
    static CURRENT: BatchId;
}

impl BatchId {
    pub fn fresh() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    pub fn get(self) -> u64 {
        self.0
    }

    /// Run `fut` as this batch: every write it makes on its own task is the
    /// batch's. A task it spawns is not.
    pub async fn scope<F: Future>(self, fut: F) -> F::Output {
        CURRENT.scope(self, fut).await
    }

    /// The batch the running task belongs to, if any.
    pub fn current() -> Option<Self> {
        // ALLOW(ok): the only error is "not inside a scope", which is the `None`
        // answer.
        CURRENT.try_with(|id| *id).ok()
    }
}

/// A block a write that was not the batch's created or edited inside the
/// window, and that the rollback took with it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LostWrite {
    pub doc: String,
    pub block: String,
}

/// A completed rollback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolledBack {
    pub undone_steps: usize,
    pub lost: Vec<LostWrite>,
}

/// A rollback the authority did not complete.
///
/// Every variant but [`Self::Incomplete`] is decided before anything is
/// undone, so the store is exactly as the failed batch left it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RollbackRefused {
    #[error("{doc} could not be reached to roll it back: {detail}")]
    Unreachable { doc: String, detail: String },
    #[error(
        "the batch wrote into {doc}, but its undo history does not hold those writes: {detail}"
    )]
    Unrecorded { doc: String, detail: String },
    #[error(
        "{ops} op(s) were dispatched, but the write authority recorded no write as this batch's, \
         so there is nothing it can take back"
    )]
    NothingRecorded { ops: usize },
    #[error(
        "the rollback undid {reverted:?} and then could not undo {doc}: {detail} — the store is \
         in neither the pre-batch nor the post-batch state"
    )]
    Incomplete {
        reverted: Vec<String>,
        doc: String,
        detail: String,
    },
}

/// Opening a batch whose writes the block write authority can take back.
#[async_trait]
pub trait BatchRollback: Send + Sync {
    /// Start recording, in every document a block write can reach, the writes
    /// made inside the returned batch's [`BatchId::scope`].
    async fn open(&self) -> crate::Result<Box<dyn OpenBatch>>;
}

/// A batch being recorded. Dropping it ends the recording.
#[async_trait]
pub trait OpenBatch: Send + Sync {
    fn id(&self) -> BatchId;

    /// Undo every write the batch made, keeping every write that was not its.
    async fn roll_back(self: Box<Self>) -> Result<RolledBack, RollbackRefused>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_batch_is_current_only_on_its_own_task() {
        let id = BatchId::fresh();
        assert_eq!(BatchId::current(), None);
        id.scope(async {
            assert_eq!(BatchId::current(), Some(id));
            let spawned = tokio::spawn(async { BatchId::current() }).await.unwrap();
            assert_eq!(spawned, None, "a spawned task is not the batch");
        })
        .await;
        assert_eq!(BatchId::current(), None);
    }

    #[test]
    fn every_batch_gets_its_own_id() {
        assert_ne!(BatchId::fresh(), BatchId::fresh());
    }
}
