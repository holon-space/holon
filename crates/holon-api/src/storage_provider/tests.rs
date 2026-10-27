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
