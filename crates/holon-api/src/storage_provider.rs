//! The lifecycle contract every storage provider (Turso, Loro, a plugin's
//! store) satisfies, so the net's storage subnet can open them, let a standby
//! catch up, swap it in and retire the old one without knowing which store it
//! holds (net semantics revision 2, holon-work #171/#315).
//!
//! Two things deliberately do NOT live here:
//! - **Locking.** "Apply the final delta while held" is a precondition the net
//!   enforces with holder tokens (queries shared, writes/swap/retirement
//!   exclusive), never a lock inside a provider.
//! - **Retries and priority.** Both are token data in the net (`tries` per
//!   provider on the request token, priority on the `Service<X>` token), so a
//!   provider has no retry state and no rank.

use std::time::Duration;

use async_trait::async_trait;

/// How far a store has been written forward, in writes accepted by the vault
/// this provider serves.
///
/// Monotone and comparable only between providers of the SAME vault: the swap
/// guard compares a standby's version with the primary's and fires only on
/// equality. The number has no meaning across vaults or across devices.
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

    /// Apply the writes this store is missing, up to `target`, and stop after
    /// `budget` even when more are left.
    ///
    /// Where the missing writes come from is wired when the provider is built
    /// (the vault it serves), so a caller never names a source.
    ///
    /// Returns the version actually reached. Reaching less than `target` is a
    /// normal result, not a failure: the caller (background catch-up) runs it
    /// again, or (the swap, holding the store token exclusively so `target`
    /// cannot move) releases the hold and lets the swap guard stay false.
    ///
    /// Postcondition on `Ok(v)`: `v <= target`, `v >= version()` before the
    /// call, and `version() == v` after it.
    async fn catch_up(
        &self,
        target: StorageVersion,
        budget: Duration,
    ) -> Result<StorageVersion, ProviderFailure>;

    /// Release the store's resources.
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
    /// that is always able to reach `target`, bounded by `budget` only in that
    /// it applies one write per millisecond of budget.
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
            budget: Duration,
        ) -> Result<StorageVersion, ProviderFailure> {
            let reachable = self.version().0 + budget.as_millis() as u64;
            let reached = reachable.min(target.0).max(self.version().0);
            self.applied.store(reached, Ordering::SeqCst);
            Ok(StorageVersion(reached))
        }

        async fn close(&self) -> Result<(), ProviderFailure> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_bounded_catch_up_reports_the_version_it_reached() {
        let store = FakeStore {
            applied: AtomicU64::new(0),
        };
        assert_eq!(store.open().await.unwrap(), StorageVersion(0));

        let partial = store
            .catch_up(StorageVersion(10), Duration::from_millis(4))
            .await
            .unwrap();
        assert_eq!(partial, StorageVersion(4), "the budget bounds the delta");
        assert_eq!(store.version(), partial);

        let caught = store
            .catch_up(StorageVersion(10), Duration::from_millis(100))
            .await
            .unwrap();
        assert_eq!(caught, StorageVersion(10), "never past the target");
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
