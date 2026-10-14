//! The lifecycle contract every storage provider (Turso, Loro, a plugin's
//! store) satisfies, so the net's storage subnet can open them, let a standby
//! catch up, swap it in and retire the old one without knowing which store it
//! holds (net semantics revision 2, holon-work #171/#315).
//!
//! A swappable pair is a **projection pair**: one store is the source (Loro)
//! and the other a projection of it (Turso). Catching up therefore means
//! replaying the source's commits, and the source is wired when a provider is
//! built. Pairs that are not projections of one another are out of scope.
//!
//! Three things deliberately do NOT live here:
//! - **Locking.** "Apply the final delta while held" is a precondition the net
//!   enforces with holder tokens (queries shared, writes/swap/retirement
//!   exclusive), never a lock inside a provider.
//! - **Retries and priority.** Both are token data in the net (`tries` keyed by
//!   the contribution's stable id on the request token, priority on the
//!   `Service<X>` token), so a provider has no retry state and no rank.
//! - **Time bounds.** A run timer on the transition is the net's deadline; a
//!   provider is bounded by a write COUNT, which it can honour at a commit
//!   boundary.

use std::num::NonZeroU64;

use async_trait::async_trait;

/// A position in the source's commit sequence on this device: monotone,
/// persisted, one step per committed delta including an incoming sync.
///
/// Opaque. Comparable only between the two providers of ONE projection pair on
/// ONE device — the swap guard compares the standby's position with the
/// primary's. It is not a `WriteSeq` (that one orders this process's editor
/// writes) and has no meaning across devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StorageVersion(pub u64);

/// A provider attempt that did not reach its goal, split by what the caller may
/// conclude — the distinction the executor needs to turn a failed body into
/// either a definite failure (the input returns with `tries + 1`) or an unknown
/// outcome (the occurrence parks and a receiver resolves it).
#[derive(Debug, thiserror::Error)]
pub enum ProviderFailure {
    /// The attempt did not take effect. The store is in the state it had
    /// before, so the caller may count a try and attempt again.
    #[error("definite failure: {0:#}")]
    Definite(anyhow::Error),
    /// The attempt MAY have taken effect — a timeout, a lost connection, a
    /// crashed peer. Attempting again without a receiver may apply it twice.
    #[error("unknown outcome, the attempt may have taken effect: {0:#}")]
    Unknown(anyhow::Error),
}

/// Open, catch up, close — the four things the storage subnet does to a store.
///
/// Implemented on the shared provider handle (`&self`, interior mutability),
/// because the net holds providers as tokens and several holders read one store
/// at a time.
#[async_trait]
pub trait StorageProvider: Send + Sync {
    /// The store's name in disclosed messages ("Turso", "Loro"). The net holds
    /// providers behind a trait object and could not name them otherwise.
    fn name(&self) -> &str;

    /// Make the store usable and report the version it starts from.
    ///
    /// Postcondition on `Ok`: [`Self::version`] is callable and returns the
    /// returned version.
    async fn open(&self) -> Result<StorageVersion, ProviderFailure>;

    /// The newest write this store has applied. Cheap and infallible: the swap
    /// guard reads it on every evaluation.
    ///
    /// Precondition: [`Self::open`] returned `Ok` and [`Self::close`] has not
    /// been called. Postcondition: never decreases.
    fn version(&self) -> StorageVersion;

    /// Replay the source's commits this store is missing, up to `target`, and
    /// at most `max_writes` of them.
    ///
    /// Which source to replay is wired when the provider is built, so a caller
    /// never names one.
    ///
    /// Postcondition on `Ok(v)`, with `before` the version at entry:
    /// `v == min(target, before + max_writes)`, and `version() == v` after the
    /// call. The result is therefore determined by the arguments: a provider
    /// that cannot replay that many commits returns a failure instead of a
    /// short count.
    ///
    /// Both callers rely on exactly that. Background catch-up runs it until the
    /// lag is within the swap guard's bound. The swap holds the store token
    /// exclusively, so `target` cannot move and a `max_writes` at or above the
    /// lag MUST reach `target`; a shortfall is a broken postcondition that the
    /// swap attempt reports as a definite failure (`tries + 1` on the swap
    /// request) — never a silent release of the hold.
    async fn catch_up(
        &self,
        target: StorageVersion,
        max_writes: NonZeroU64,
    ) -> Result<StorageVersion, ProviderFailure>;

    /// Flush whatever must survive and release the store's resources. There is
    /// no separate `drain`: this is it.
    ///
    /// Precondition: no holder uses it any more — retirement is an exclusive
    /// holder. After `Ok`, every other method's precondition is violated.
    async fn close(&self) -> Result<(), ProviderFailure>;
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;

    use super::*;

    /// Proves the trait is implementable against ordinary shared state: a store
    /// that replays every commit it is asked for, up to `max_writes`.
    struct FakeStore {
        applied: AtomicU64,
    }

    #[async_trait]
    impl StorageProvider for FakeStore {
        fn name(&self) -> &str {
            "Fake"
        }

        async fn open(&self) -> Result<StorageVersion, ProviderFailure> {
            Ok(self.version())
        }

        fn version(&self) -> StorageVersion {
            StorageVersion(self.applied.load(Ordering::SeqCst))
        }

        async fn catch_up(
            &self,
            target: StorageVersion,
            max_writes: NonZeroU64,
        ) -> Result<StorageVersion, ProviderFailure> {
            let before = self.version().0;
            let reached = target.0.min(before + max_writes.get()).max(before);
            self.applied.store(reached, Ordering::SeqCst);
            Ok(StorageVersion(reached))
        }

        async fn close(&self) -> Result<(), ProviderFailure> {
            Ok(())
        }
    }

    fn writes(n: u64) -> NonZeroU64 {
        NonZeroU64::new(n).expect("a catch-up budget is at least one write")
    }

    #[tokio::test]
    async fn a_bounded_catch_up_reaches_exactly_min_of_target_and_budget() {
        let store = FakeStore {
            applied: AtomicU64::new(0),
        };
        assert_eq!(store.open().await.unwrap(), StorageVersion(0));

        let partial = store.catch_up(StorageVersion(10), writes(4)).await.unwrap();
        assert_eq!(
            partial,
            StorageVersion(4),
            "the write count bounds the delta"
        );
        assert_eq!(store.version(), partial);

        let caught = store
            .catch_up(StorageVersion(10), writes(100))
            .await
            .unwrap();
        assert_eq!(
            caught,
            StorageVersion(10),
            "a budget at or above the lag reaches the target exactly"
        );
        store.close().await.unwrap();
    }

    #[test]
    fn a_failure_discloses_whether_the_attempt_may_have_taken_effect() {
        let definite = ProviderFailure::Definite(anyhow::anyhow!("lock held by another process"));
        assert!(
            definite
                .to_string()
                .contains("lock held by another process")
        );
        let unknown = ProviderFailure::Unknown(anyhow::anyhow!("open timed out"));
        assert!(unknown.to_string().contains("may have taken effect"));
    }
}
