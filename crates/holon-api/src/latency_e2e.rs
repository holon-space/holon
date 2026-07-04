//! End-to-end interaction latency: dispatch → rows-visible (`stage="e2e"`).
//!
//! The per-stage `holon_latency` events (`dispatch`, `projection`, `rows`)
//! measure pipeline components in isolation; none measures what the user
//! feels. This module correlates the two ends of the prod pipeline by
//! **target entity id** across the async task boundary:
//!
//! - [`interaction_dispatched`] — called at the operation-dispatch entry point
//!   (`holon-frontend` `dispatch_intent{,_sync}`) with the op name and the
//!   target entity id. Starts the clock.
//! - [`rows_delivered`] — called from `LiveData::subscribe` (the shared engine
//!   mirror that BOTH the default SQL path and the CRDT projection path feed
//!   through CDC) with the ids touched by an applied batch. The first batch
//!   whose ids contain a pending target (as row id or as `parent_id` of a
//!   created/updated row — covers create/split ops whose new row is parented
//!   under the target) closes the clock and emits:
//!
//!   `tracing::debug!(target="holon_latency", stage="e2e", action, block,
//!   source, ms)`
//!
//! Boundary disclosures:
//! - Final GPU paint is out of scope (same as every other stage).
//! - Ops without an `id` param, and deletes whose CDC `Deleted.id` is a rowid
//!   rather than the entity id, are not correlated (no event).
//! - Entries expire after 30s (op failed / never touched its target row).
//!
//! Cost: one small mutex op per dispatch; per-batch cost is a single atomic
//! load while no interaction is pending (the overwhelmingly common case).

use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

struct Pending {
    action: String,
    target: String,
    t0: Instant,
}

static PENDING_LEN: AtomicUsize = AtomicUsize::new(0);
static PENDING: Mutex<Vec<Pending>> = Mutex::new(Vec::new());
const MAX_PENDING: usize = 64;
const EXPIRY: Duration = Duration::from_secs(30);

/// Record a user interaction entering the op pipeline. `target` is the
/// entity the op addresses (the `id` param).
pub fn interaction_dispatched(action: &str, target: &str) {
    let mut pending = PENDING.lock().expect("latency_e2e mutex poisoned");
    let now = Instant::now();
    pending.retain(|e| now.duration_since(e.t0) < EXPIRY);
    if pending.len() >= MAX_PENDING {
        pending.remove(0);
    }
    pending.push(Pending {
        action: action.to_string(),
        target: target.to_string(),
        t0: now,
    });
    PENDING_LEN.store(pending.len(), Ordering::Release);
}

/// Report the entity ids made visible by an applied `LiveData` batch.
/// Emits one `stage="e2e"` event per pending interaction matched.
pub fn rows_delivered<'a>(source: &'static str, ids: impl IntoIterator<Item = &'a str>) {
    if PENDING_LEN.load(Ordering::Acquire) == 0 {
        return;
    }
    let id_set: std::collections::HashSet<&str> = ids.into_iter().collect();
    if id_set.is_empty() {
        return;
    }
    let mut pending = PENDING.lock().expect("latency_e2e mutex poisoned");
    let mut i = 0;
    while i < pending.len() {
        if id_set.contains(pending[i].target.as_str()) {
            let done = pending.remove(i);
            let ms = done.t0.elapsed().as_millis() as u64;
            // End-to-end (interaction -> visible-to-render): the prod
            // counterpart of the harness-only `action_total` stage.
            tracing::debug!(
                target: "holon_latency",
                stage = "e2e",
                action = %done.action,
                block = %done.target,
                source = source,
                ms = ms,
                "holon_latency",
            );
        } else {
            i += 1;
        }
    }
    PENDING_LEN.store(pending.len(), Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_by_target_id_and_clears() {
        // NOTE: the registry is process-global and tests run in parallel —
        // assert only on this test's own entry, never on global emptiness.
        interaction_dispatched("toggle_state", "block:e2e-test-a");
        rows_delivered("block", ["block:other", "block:e2e-test-a"]);
        let pending = PENDING.lock().unwrap();
        assert!(
            pending.iter().all(|p| p.target != "block:e2e-test-a"),
            "matched entry must be consumed"
        );
    }

    #[test]
    fn unmatched_ids_leave_entry_pending_until_expiry() {
        interaction_dispatched("set_field", "block:e2e-test-b");
        rows_delivered("block", ["block:unrelated"]);
        let pending = PENDING.lock().unwrap();
        assert!(pending.iter().any(|p| p.target == "block:e2e-test-b"));
    }
}
