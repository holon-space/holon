use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

/// Wall-clock authority. Passed as a typed value so timestamps are
/// deterministic in tests instead of reaching for the ambient system clock.
pub trait Clock: Send + Sync + std::fmt::Debug {
    /// Milliseconds since the Unix epoch.
    fn now_millis(&self) -> i64;
}

/// Production clock: reads the real system time.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_millis(&self) -> i64 {
        // Preserves the exact pre-existing call used across block constructors.
        chrono::Utc::now().timestamp_millis()
    }
}

/// Deterministic clock for tests. Cheap to clone; clones share the same
/// instant.
#[derive(Debug, Clone)]
pub struct TestClock {
    millis: Arc<AtomicI64>,
}

impl TestClock {
    pub fn new(start_millis: i64) -> Self {
        Self {
            millis: Arc::new(AtomicI64::new(start_millis)),
        }
    }
    pub fn set(&self, millis: i64) {
        self.millis.store(millis, Ordering::SeqCst);
    }
    /// Advance by `delta_millis` and return the new value.
    pub fn advance(&self, delta_millis: i64) -> i64 {
        self.millis.fetch_add(delta_millis, Ordering::SeqCst) + delta_millis
    }
}

impl Clock for TestClock {
    fn now_millis(&self) -> i64 {
        self.millis.load(Ordering::SeqCst)
    }
}

/// Free helper for sites that have no clock to inject (free constructors,
/// `Default`). Routes through [`SystemClock`] — the single chokepoint that a
/// future change can make injectable. Prefer holding a `Clock` where a `self`
/// exists (see `SqlOperationProvider`).
pub fn now_millis() -> i64 {
    SystemClock.now_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_clock_is_deterministic() {
        let c = TestClock::new(1_000);
        assert_eq!(c.now_millis(), 1_000);
        assert_eq!(c.advance(500), 1_500);
        assert_eq!(c.now_millis(), 1_500);
        c.set(42);
        assert_eq!(c.now_millis(), 42);
    }
    #[test]
    fn system_clock_is_positive() {
        assert!(SystemClock.now_millis() > 0);
    }
}
