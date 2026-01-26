//! Per-handle subscription registry for B3 `watch_view` / `drop_subscription`.
//!
//! Each `watch_view` call allocates a handle, spawns a tokio task on the
//! worker's `current_thread` runtime that drains
//! `ReactiveEngine::watch(block_id)`, and installs the task's `JoinHandle`
//! in the registry under that handle. The task is responsible for
//! removing its own entry when the stream ends naturally; `cancel` aborts
//! and removes on user request.
//!
//! Handle allocation is split from installation so the spawned task can
//! capture its handle up-front — no reliance on TSFN-is-async ordering.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use tokio::task::JoinHandle;

static NEXT_HANDLE: AtomicU32 = AtomicU32::new(1);

static REGISTRY: OnceLock<Mutex<HashMap<u32, JoinHandle<()>>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<u32, JoinHandle<()>>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Allocate a fresh handle without yet having a task to store. The caller
/// must follow up with `install` before the handle is observable to users.
pub fn allocate() -> u32 {
    let id = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    // u32 wraparound: ~4B allocations. If NEXT_HANDLE hits 0 (sentinel) or
    // collides with a live entry, something has gone catastrophically wrong —
    // panic loud rather than silently overwrite a live subscription.
    assert!(id != 0, "subscription handle counter wrapped to zero");
    let guard = registry().lock();
    assert!(
        !guard.contains_key(&id),
        "subscription handle {id} already in use — counter wrapped",
    );
    id
}

/// Install a task under a previously-allocated handle.
pub fn install(handle: u32, task: JoinHandle<()>) {
    let prev = registry().lock().insert(handle, task);
    assert!(prev.is_none(), "install called twice for handle {handle}",);
}

/// Remove the registry entry for `handle` without aborting. Used by the
/// drain task itself when its stream ends naturally.
pub fn remove(handle: u32) {
    registry().lock().remove(&handle);
}

/// Abort and remove the task for `handle`. No-op if already dropped.
pub fn cancel(handle: u32) {
    if let Some(task) = registry().lock().remove(&handle) {
        task.abort();
    }
}
