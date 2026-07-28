//! Boot-ordering gate that keeps integration/provider syncs off the serialized
//! `DatabaseActor` until the org initial scan has finished.
//!
//! The single-consumer `DatabaseActor` serializes every write. During boot the
//! org `FileSyncController` initial scan saturates it ingesting the whole
//! vault; an MCP/provider sync running concurrently interleaves full re-syncs
//! into the same queue and starves the scan (measured: 79 claude-history
//! re-syncs during a 4-minute boot, per-file scan stalls of 6-25s). The gate
//! makes provider syncs *wait* for scan completion, then run once against an
//! idle actor.

use std::sync::Arc;

use tokio::sync::watch;

/// Whether integration/provider syncs may run yet.
///
/// Starts `DeferredUntilScan`; flips to `Open` exactly once, when the org
/// initial scan completes — on *every* completion path (success, per-file
/// degradation, or a fail-loud stall error), because in all three cases the
/// scan is no longer holding the actor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncGateState {
    /// Boot scan in progress — syncs must wait.
    DeferredUntilScan,
    /// Scan finished — syncs may run.
    Open,
}

/// Raised by [`SyncGate::wait_open`] when every gate holder was dropped before
/// the gate opened (process teardown). Callers proceed in disclosed-degraded
/// mode rather than block forever.
#[derive(Debug, thiserror::Error)]
#[error("sync gate closed without opening — all holders dropped (shutdown)")]
pub struct SyncGateClosed;

/// Shared handle to the boot-ordering gate. Clone freely; every clone observes
/// the same state.
#[derive(Debug, Clone)]
pub struct SyncGate {
    tx: Arc<watch::Sender<SyncGateState>>,
}

impl Default for SyncGate {
    fn default() -> Self {
        Self::new()
    }
}

impl SyncGate {
    /// A fresh gate in the `DeferredUntilScan` state.
    pub fn new() -> Self {
        let (tx, _rx) = watch::channel(SyncGateState::DeferredUntilScan);
        Self { tx: Arc::new(tx) }
    }

    /// A gate already `Open` — for contexts with no boot scan to wait on
    /// (tests, MCP-only wiring).
    pub fn opened() -> Self {
        let gate = Self::new();
        gate.open();
        gate
    }

    /// Open the gate. Idempotent — opening an already-open gate is a no-op.
    pub fn open(&self) {
        self.tx.send_if_modified(|s| {
            if *s == SyncGateState::Open {
                false
            } else {
                *s = SyncGateState::Open;
                true
            }
        });
    }

    /// Current state (for assertions / diagnostics).
    pub fn state(&self) -> SyncGateState {
        *self.tx.borrow()
    }

    /// Resolve once the gate is `Open`. Returns immediately if already open.
    /// Errors only if all holders were dropped before opening.
    pub async fn wait_open(&self) -> std::result::Result<(), SyncGateClosed> {
        let mut rx = self.tx.subscribe();
        loop {
            if *rx.borrow_and_update() == SyncGateState::Open {
                return Ok(());
            }
            rx.changed().await.map_err(|_| SyncGateClosed)?;
        }
    }

    /// A receive-only view for waiters that must be able to observe
    /// "every gate holder is gone".
    ///
    /// [`wait_open`](Self::wait_open) borrows `self`, so a waiter that owns a
    /// `SyncGate` keeps a sender alive and its `SyncGateClosed` arm is
    /// unreachable by construction. A waiter that holds only a
    /// [`SyncGateWatcher`] can actually reach it.
    pub fn watcher(&self) -> SyncGateWatcher {
        SyncGateWatcher {
            rx: self.tx.subscribe(),
        }
    }
}

/// Receive-only handle to a [`SyncGate`]. Holding one does NOT keep the gate
/// open-able — dropping every `SyncGate` while a watcher waits resolves the
/// wait with [`SyncGateClosed`] instead of parking forever.
#[derive(Debug, Clone)]
pub struct SyncGateWatcher {
    rx: watch::Receiver<SyncGateState>,
}

impl SyncGateWatcher {
    /// Current state (for assertions / diagnostics).
    pub fn state(&self) -> SyncGateState {
        *self.rx.borrow()
    }

    /// Resolve once the gate is `Open`. Returns immediately if already open.
    pub async fn wait_open(&mut self) -> std::result::Result<(), SyncGateClosed> {
        loop {
            if *self.rx.borrow_and_update() == SyncGateState::Open {
                return Ok(());
            }
            self.rx.changed().await.map_err(|_| SyncGateClosed)?;
        }
    }
}
