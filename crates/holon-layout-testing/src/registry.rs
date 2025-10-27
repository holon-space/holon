//! Instance-owned `BlockTreeRegistry` — replaces the process-global
//! `static OnceLock<Mutex<HashMap>>` in `frontends/gpui/tests/support/mod.rs`.
//!
//! Each `GpuiScenarioSession` owns exactly one registry, constructed fresh
//! per scenario. No `static`, no cross-case contamination.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::LiveBlock;

/// A thunk that materialises a fresh `ReactiveViewModel` on demand.
pub type BlockTreeThunk = Arc<dyn Fn() -> ReactiveViewModel + Send + Sync>;

/// One registry entry per LiveBlock id exercised in a scenario.
///
/// `modes` is ordered as the VMS's `modes` JSON would be: first entry is
/// the default mode when no active-mode override is supplied.
/// `active_mode` indexes into `modes`.
///
/// `stream_tx` is `None` until `watch_live` is called at least once for
/// this block — lazily allocated so `set_active` can push new trees through
/// to wake up the downstream `ReactiveShell` consumer task.
pub struct BlockEntry {
    pub modes: Vec<(String, BlockTreeThunk)>,
    pub active_mode: usize,
    pub stream_tx: Option<futures::channel::mpsc::UnboundedSender<ReactiveViewModel>>,
}

/// A per-scenario registry mapping block IDs to their mode thunks.
///
/// Construct a fresh `BlockTreeRegistry` for each proptest scenario;
/// discard it at the end of the scenario (no `clear` method needed).
#[derive(Clone)]
pub struct BlockTreeRegistry {
    inner: Arc<Mutex<HashMap<String, BlockEntry>>>,
}

impl BlockTreeRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a mode-switchable block. `active_mode` must be < `modes.len()`.
    ///
    /// Overwrites any prior entry for the same `block_id`, including its
    /// `stream_tx` — a fresh `watch_live` call is needed before subsequent
    /// `set_active` invocations can push to the `ReactiveShell` consumer.
    pub fn register(
        &self,
        block_id: impl Into<String>,
        modes: Vec<(String, BlockTreeThunk)>,
        active_mode: usize,
    ) {
        assert!(
            active_mode < modes.len(),
            "active_mode {active_mode} out of range for {} modes",
            modes.len()
        );
        self.inner.lock().unwrap().insert(
            block_id.into(),
            BlockEntry {
                modes,
                active_mode,
                stream_tx: None,
            },
        );
    }

    /// Flip the active mode for a registered block and push a freshly-
    /// materialised tree onto the block's structural-changes stream (if
    /// `watch_live` has already been called for it).
    ///
    /// Silently ignores missing blocks and missing mode names — proptest
    /// shrinking can remove handles from a scenario while leaving actions
    /// in place, and we don't want to turn a stale action into a panic that
    /// masks the real minimal failure.
    pub fn set_active(&self, block_id: &str, mode_name: &str) {
        let mut guard = self.inner.lock().unwrap();
        let Some(entry) = guard.get_mut(block_id) else {
            return;
        };
        let Some(idx) = entry.modes.iter().position(|(n, _)| n == mode_name) else {
            return;
        };
        entry.active_mode = idx;
        let new_tree = (entry.modes[idx].1)();
        if let Some(tx) = entry.stream_tx.as_ref() {
            let _ = tx.unbounded_send(new_tree);
        }
    }

    /// Return a `LiveBlock` for the given block ID, creating a fresh mpsc
    /// channel pair and storing the sender for future `set_active` calls.
    ///
    /// Panics if `block_id` was not registered — that's a fixture bug.
    pub fn watch_live(&self, block_id: &str) -> LiveBlock {
        let (tree, stream) = {
            let mut guard = self.inner.lock().unwrap();
            let entry = guard.get_mut(block_id).unwrap_or_else(|| {
                panic!(
                    "BlockTreeRegistry::watch_live called for unregistered block_id \
                     `{block_id}`; register modes with BlockTreeRegistry::register \
                     before rendering"
                )
            });
            let (_, thunk) = &entry.modes[entry.active_mode];
            let tree = thunk();
            let (tx, rx) = futures::channel::mpsc::unbounded::<ReactiveViewModel>();
            entry.stream_tx = Some(tx);
            (tree, rx)
        };
        LiveBlock {
            tree,
            structural_changes: Box::pin(stream),
        }
    }

    /// Return the current active mode name for `block_id`, if registered.
    pub fn active_mode_name(&self, block_id: &str) -> Option<String> {
        let guard = self.inner.lock().unwrap();
        guard
            .get(block_id)
            .map(|e| e.modes[e.active_mode].0.clone())
    }
}

impl Default for BlockTreeRegistry {
    fn default() -> Self {
        Self::new()
    }
}
