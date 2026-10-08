//! Whether the boot seed may still be writing the default layout.
//!
//! The seed writes the layout one block at a time, so while it runs the store
//! can hold a perspective whose panels have not received their source and
//! render children yet. The root slot reads this to tell that half-written
//! state (pending) apart from a perspective that really has nothing to lay out
//! (an error).

use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutSeedPhase {
    Pending,
    Settled,
}

/// Shared handle to the engine's layout-seed phase. Starts
/// [`LayoutSeedPhase::Settled`]: a wiring without a boot seed never waits.
#[derive(Clone)]
pub struct LayoutSeed(watch::Sender<LayoutSeedPhase>);

impl LayoutSeed {
    pub fn new() -> Self {
        Self(watch::channel(LayoutSeedPhase::Settled).0)
    }

    /// Mark the seed as running until the returned guard drops. Call it
    /// before anything can render the root slot.
    pub fn begin(&self) -> LayoutSeedRunning {
        let previous = self.0.send_replace(LayoutSeedPhase::Pending);
        assert_eq!(
            previous,
            LayoutSeedPhase::Settled,
            "a layout seed began while another one was still running"
        );
        LayoutSeedRunning(self.0.clone())
    }

    pub fn phase(&self) -> LayoutSeedPhase {
        *self.0.borrow()
    }

    pub fn subscribe(&self) -> watch::Receiver<LayoutSeedPhase> {
        self.0.subscribe()
    }
}

impl Default for LayoutSeed {
    fn default() -> Self {
        Self::new()
    }
}

/// Settles the phase on drop, so a seed task that fails, panics or is aborted
/// still ends the pending state and the root slot shows what the store holds.
pub struct LayoutSeedRunning(watch::Sender<LayoutSeedPhase>);

impl Drop for LayoutSeedRunning {
    fn drop(&mut self) {
        self.0.send_replace(LayoutSeedPhase::Settled);
    }
}
