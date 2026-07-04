//! Actor-owned lifetime for `watch_view_*` materialized views.
//!
//! A matview is only useful while something consumes its CDC stream, but its
//! DBSP circuit is walked on every commit whether or not anyone listens. So a
//! watch view is held open by *leases*: a subscriber acquires one and the view
//! is dropped once the last one is released.
//!
//! Every field here is owned by the single-threaded database actor and mutated
//! only between command executions. "Count a lease" and "drop the view" are
//! therefore separate, uninterruptible steps on one queue — no lock, and no
//! window in which a view is both leased and being reaped.

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use holon_core::storage::Result;
use holon_core::storage::StorageError;
use tokio::sync::oneshot;

/// Proof that its holder keeps one materialized view alive.
///
/// `generation` pins the grant to the reset epoch it was issued in: a release
/// racing a `ResetWatchViews` names a view of a bygone epoch and is discarded
/// instead of decrementing a freshly created one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeaseGrant {
    pub lease_id: u64,
    pub generation: u64,
}

/// Lease counters the actor republishes after every mutation, so a `DbHandle`
/// reads them without an actor round trip.
#[derive(Debug, Default)]
pub struct MatviewStats {
    leased_views: AtomicU64,
    active_leases: AtomicU64,
    pinned: AtomicU64,
}

/// Sample of [`MatviewStats`]. Coherent as a whole: the actor publishes all
/// three fields from one map walk, between commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatviewStatsSnapshot {
    /// Views the actor owns, whether `Creating` or `Live`.
    pub leased_views: u64,
    /// Sum of the live lease counts over all owned views.
    pub active_leases: u64,
    /// Owned views held open by a pin.
    pub pinned: u64,
}

impl MatviewStats {
    pub fn snapshot(&self) -> MatviewStatsSnapshot {
        MatviewStatsSnapshot {
            leased_views: self.leased_views.load(Ordering::Relaxed),
            active_leases: self.active_leases.load(Ordering::Relaxed),
            pinned: self.pinned.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn publish(&self, views: &HashMap<String, ViewState>) {
        let mut leased_views = 0u64;
        let mut active_leases = 0u64;
        let mut pinned = 0u64;
        for state in views.values() {
            leased_views += 1;
            if let ViewState::Live {
                leases,
                pinned: is_pinned,
            } = state
            {
                active_leases += u64::from(*leases);
                if *is_pinned {
                    pinned += 1;
                }
            }
        }
        self.leased_views.store(leased_views, Ordering::Relaxed);
        self.active_leases.store(active_leases, Ordering::Relaxed);
        self.pinned.store(pinned, Ordering::Relaxed);
    }
}

/// A caller parked on a view whose `CREATE` has not completed yet.
pub(crate) enum ViewWaiter {
    Lease(oneshot::Sender<Result<LeaseGrant>>),
    Pin(oneshot::Sender<Result<()>>),
}

impl ViewWaiter {
    pub(crate) fn is_pin(&self) -> bool {
        matches!(self, Self::Pin(_))
    }

    /// Answer this waiter with a failure. The receiver may already be gone
    /// (caller timed out or was cancelled); that is not itself an error.
    pub(crate) fn fail(self, message: String) {
        match self {
            Self::Lease(tx) => {
                let _ = tx.send(Err(StorageError::DatabaseError(message)));
            }
            Self::Pin(tx) => {
                let _ = tx.send(Err(StorageError::DatabaseError(message)));
            }
        }
    }
}

pub(crate) enum ViewState {
    /// The `CREATE MATERIALIZED VIEW` is in flight — possibly parked on the
    /// deferred-DDL queue waiting for a base table. Every waiter is answered
    /// when it completes.
    Creating {
        waiters: Vec<ViewWaiter>,
        pin_requested: bool,
    },
    /// The view exists. It is reaped when `leases` reaches zero, unless
    /// `pinned` — a pin outlives any number of later lease cycles.
    Live { leases: u32, pinned: bool },
}
