//! Publish-error accounting shared across event adapters.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Tracks publish errors from event adapters.
///
/// This is useful for detecting "Database schema changed" errors that occur
/// during startup when DDL operations (like preload_views) race with writes
/// from sync adapters.
///
/// Register this in DI and share it across adapters to track errors
/// without relying on log scraping.
#[derive(Clone, Default)]
pub struct PublishErrorTracker {
    /// Count of failed publish attempts
    error_count: Arc<AtomicUsize>,
    /// Count of successful publish attempts
    success_count: Arc<AtomicUsize>,
}

impl PublishErrorTracker {
    pub fn new() -> Self {
        Self {
            error_count: Arc::new(AtomicUsize::new(0)),
            success_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Record a publish error
    pub fn record_error(&self) {
        self.error_count.fetch_add(1, Ordering::SeqCst);
    }

    /// Record a successful publish
    pub fn record_success(&self) {
        self.success_count.fetch_add(1, Ordering::SeqCst);
    }

    /// Get the number of publish errors
    pub fn errors(&self) -> usize {
        self.error_count.load(Ordering::SeqCst)
    }

    /// Get the number of successful publishes
    pub fn successes(&self) -> usize {
        self.success_count.load(Ordering::SeqCst)
    }

    /// Returns true if any publish errors occurred
    pub fn has_errors(&self) -> bool {
        self.errors() > 0
    }

    /// Get total attempts (errors + successes)
    pub fn total_attempts(&self) -> usize {
        self.errors() + self.successes()
    }

    /// Reset counters (useful for tests)
    pub fn reset(&self) {
        self.error_count.store(0, Ordering::SeqCst);
        self.success_count.store(0, Ordering::SeqCst);
    }
}
