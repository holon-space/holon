//! End-to-end interaction latency: dispatch → rows-visible (`stage="e2e"`).
//!
//! The per-stage `holon_latency` events (`dispatch`, `projection`, `rows`)
//! measure pipeline components in isolation; none measures what the user
//! feels. This module correlates the two ends of the prod pipeline across the
//! async task boundary and emits the wall time from interaction to visible row.
//!
//! # Correlation identity — why FIFO-by-target is wrong
//!
//! An interaction addresses a **target** entity (the op's `id` param). The
//! naive correlator matched a delivered CDC batch to the *oldest* pending
//! entry for that target (FIFO). That mis-attributes whenever a dispatch
//! produces **no CDC delta** — e.g. a coalesced / identity re-commit (the
//! editor's blur re-commit of already-stored content). The no-op leaves a
//! pending entry that squats until its 30s expiry and then *steals* the match
//! from the next real commit on the same block, reporting that stale entry's
//! huge elapsed as the e2e latency. This flooded the SLO oracle with phantom
//! `>10s` banners on pipelines whose real p95 was ~30ms (BugFunnel 2026-07-13).
//!
//! The fix correlates by **op instance**, not FIFO:
//!
//! - Editor content writes already carry a [`WriteSeq`] token (process-global,
//!   monotonic; stamped into the op params, round-tripped through
//!   `block_raw.write_seq`, and projected into the block matview → it reaches
//!   the delivered CDC row). Where a token is present on both ends we close
//!   **exactly** the entry whose `WriteSeq` the delivered row carries. A no-op
//!   commit's token never appears in any delta, so it can never be closed and
//!   can never steal — it is dropped the moment a newer commit on its target
//!   completes, or it expires harmlessly.
//! - Ops that carry no token (toggle / split / delete / create-by-parent) fall
//!   back to closing the **newest** pending entry for the target and dropping
//!   all older same-target entries as superseded no-ops.
//!
//! In both paths the invariant holds: a measurement is **never** attributed
//! across a newer, already-completed entry on the same target — so the SLO
//! oracle (which merely reads the emitted `e2e` events) can never fire on a
//! mis-attributed measurement.
//!
//! - [`interaction_dispatched`] — called at the operation-dispatch entry point
//!   (`holon-frontend` `dispatch_operation` / `dispatch_intent{,_sync}`) with
//!   the op name, the target entity id, and the op's `WriteSeq` if it carries
//!   one. Starts the clock.
//! - [`rows_delivered`] — called from `LiveData::subscribe` with the `(id,
//!   WriteSeq?)` pairs an applied batch made visible. Closes the matching
//!   entries and emits, per closure:
//!
//!   `tracing::debug!(target="holon_latency", stage="e2e", action, block,
//!   source, ms)`
//!
//! Boundary disclosures:
//! - Final GPU paint is out of scope (same as every other stage).
//! - Ops without an `id` param, and deletes whose CDC `Deleted.id` is a rowid
//!   rather than the entity id, are not correlated (no event).
//! - Entries expire after 30s (op failed / never touched its target row). With
//!   identity correlation an unexpired no-op entry is inert — it cannot steal.
//!
//! Cost: one small mutex op per dispatch; per-batch cost is a single atomic
//! load while no interaction is pending (the overwhelmingly common case).

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use crate::write_seq::WriteSeq;

struct Pending {
    action: String,
    target: String,
    /// The op-instance token, when the dispatched op carried one (editor
    /// content writes). `None` for tokenless ops (toggle/split/delete/...),
    /// which correlate by newest-per-target instead.
    seq: Option<WriteSeq>,
    t0: Instant,
}

/// One closed correlation, ready to emit. Returned by the pure
/// [`close_delivered`] core so the matching logic is testable without the
/// tracing/global plumbing.
struct Closed {
    action: String,
    target: String,
    ms: u64,
}

static PENDING_LEN: AtomicUsize = AtomicUsize::new(0);
static PENDING: Mutex<Vec<Pending>> = Mutex::new(Vec::new());
const MAX_PENDING: usize = 64;
const EXPIRY: Duration = Duration::from_secs(30);

/// Extract the op-instance token from op params: the editor stamps `write_seq`
/// (`> 0`) into content-write params; every other op omits it. A `0` value is
/// the row default (never editor-written) and is treated as absent.
pub fn write_seq_from_params(params: &HashMap<String, crate::Value>) -> Option<WriteSeq> {
    params
        .get("write_seq")
        .and_then(|v| v.as_i64())
        .filter(|&s| s > 0)
        .map(WriteSeq::from_i64)
}

/// Record a user interaction entering the op pipeline. `target` is the entity
/// the op addresses (the `id` param). `seq` is the op-instance token if the op
/// carries one (editor content writes stamp `write_seq`); `None` otherwise.
pub fn interaction_dispatched(action: &str, target: &str, seq: Option<WriteSeq>) {
    let mut pending = PENDING.lock().expect("latency_e2e mutex poisoned");
    let now = Instant::now();
    pending.retain(|e| now.duration_since(e.t0) < EXPIRY);
    if pending.len() >= MAX_PENDING {
        pending.remove(0);
    }
    pending.push(Pending {
        action: action.to_string(),
        target: target.to_string(),
        seq,
        t0: now,
    });
    PENDING_LEN.store(pending.len(), Ordering::Release);
}

/// Report the entities made visible by an applied `LiveData` batch, as
/// `(id, WriteSeq?)` pairs. `WriteSeq` is `Some` only for a row that carries a
/// real editor token (`write_seq > 0`); a `parent_id` correlation or a
/// tokenless row passes `None`. Emits one `stage="e2e"` event per closed entry.
pub fn rows_delivered<'a>(
    source: &'static str,
    deliveries: impl IntoIterator<Item = (&'a str, Option<WriteSeq>)>,
) {
    if PENDING_LEN.load(Ordering::Acquire) == 0 {
        return;
    }
    let deliveries: Vec<(String, Option<WriteSeq>)> = deliveries
        .into_iter()
        .map(|(id, seq)| (id.to_string(), seq))
        .collect();
    if deliveries.is_empty() {
        return;
    }
    let mut pending = PENDING.lock().expect("latency_e2e mutex poisoned");
    let closed = close_delivered(&mut pending, &deliveries, Instant::now());
    PENDING_LEN.store(pending.len(), Ordering::Release);
    drop(pending);
    for c in closed {
        // End-to-end (interaction -> visible-to-render): the prod counterpart
        // of the harness-only `action_total` stage.
        tracing::debug!(
            target: "holon_latency",
            stage = "e2e",
            action = %c.action,
            block = %c.target,
            source = source,
            ms = c.ms,
            "holon_latency",
        );
    }
}

/// Pure correlation core: close the pending entries a batch's deliveries
/// identify, returning the measurements to emit. Operates on a caller-owned
/// `Vec` so it is hermetic under the parallel test runner.
///
/// Per target present in the batch:
/// 1. **Exact op-instance match** — if the batch delivered a `WriteSeq` equal
///    to some pending entry's token, that entry is the winner (the newest such
///    on a tie). This closes exactly the op instance that produced the delta.
/// 2. **Tokenless fallback** — else, if the batch delivered any tokenless row
///    for the target, the winner is the *newest* pending entry for that target.
/// 3. Otherwise (only non-matching tokens) nothing is closed — the delta
///    belongs to an untracked dispatch; mis-attributing it would be worse than
///    emitting nothing.
///
/// The winner and every pending entry for the target **older than** it are
/// removed (superseded no-ops). Entries newer than the winner remain pending
/// for their own delta. This guarantees a measurement is never attributed
/// across a newer completed entry on the same target.
fn close_delivered<S: AsRef<str>>(
    pending: &mut Vec<Pending>,
    deliveries: &[(S, Option<WriteSeq>)],
    now: Instant,
) -> Vec<Closed> {
    let mut by_target: HashMap<&str, (Vec<WriteSeq>, bool)> = HashMap::new();
    for (id, seq) in deliveries {
        let entry = by_target.entry(id.as_ref()).or_insert((Vec::new(), false));
        match seq {
            Some(s) => entry.0.push(*s),
            None => entry.1 = true,
        }
    }

    let mut closed = Vec::new();
    for (target, (seqs, has_tokenless)) in by_target {
        // 1. exact op-instance match — newest entry whose token was delivered.
        let mut winner: Option<usize> = None;
        for (i, p) in pending.iter().enumerate() {
            if p.target != target {
                continue;
            }
            let matches = p.seq.is_some_and(|s| seqs.contains(&s));
            if matches && winner.is_none_or(|w| p.t0 > pending[w].t0) {
                winner = Some(i);
            }
        }
        // 2. tokenless fallback — newest entry for the target (any token state).
        if winner.is_none() && has_tokenless {
            for (i, p) in pending.iter().enumerate() {
                if p.target != target {
                    continue;
                }
                if winner.is_none_or(|w| p.t0 > pending[w].t0) {
                    winner = Some(i);
                }
            }
        }
        let Some(winner) = winner else {
            continue;
        };
        let winner_t0 = pending[winner].t0;
        closed.push(Closed {
            action: pending[winner].action.clone(),
            target: pending[winner].target.clone(),
            ms: now.duration_since(winner_t0).as_millis() as u64,
        });
        // Remove the winner and all OLDER same-target entries (superseded).
        pending.retain(|p| p.target != target || p.t0 > winner_t0);
    }
    closed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pend(action: &str, target: &str, seq: Option<i64>, t0: Instant) -> Pending {
        Pending {
            action: action.to_string(),
            target: target.to_string(),
            seq: seq.map(WriteSeq::from_i64),
            t0,
        }
    }

    /// The phantom-measurement steal (BugFunnel 2026-07-13): a no-op dispatch
    /// leaves a stale pending entry; the next real commit on the same block
    /// must be measured as ITS OWN fast latency, not the stale entry's huge
    /// elapsed. Old FIFO-by-target code returned the oldest (stale) entry.
    #[test]
    fn real_commit_not_stolen_by_stale_noop_entry() {
        let base = Instant::now();
        // A: no-op blur re-commit (its write_seq never lands in any delta).
        // B: the real edit 16s later, its own newer token.
        let mut pending = vec![
            pend("set_field", "block:x", Some(100), base),
            pend(
                "set_field",
                "block:x",
                Some(101),
                base + Duration::from_secs(16),
            ),
        ];
        // The real edit's delta carries token 101.
        let now = base + Duration::from_secs(16) + Duration::from_millis(10);
        let closed = close_delivered(
            &mut pending,
            &[("block:x", Some(WriteSeq::from_i64(101)))],
            now,
        );
        assert_eq!(closed.len(), 1, "exactly one measurement");
        assert_eq!(closed[0].action, "set_field");
        assert_eq!(
            closed[0].ms, 10,
            "must report the REAL 10ms commit, not the stale 16s no-op"
        );
        assert!(
            pending.is_empty(),
            "the matched entry and the superseded stale no-op are both cleared"
        );
    }

    /// A delivered token that matches NO pending entry closes nothing (the
    /// delta belongs to an untracked dispatch) — never mis-attribute it to a
    /// stale same-target entry.
    #[test]
    fn nonmatching_token_closes_nothing() {
        let base = Instant::now();
        let mut pending = vec![pend("set_field", "block:x", Some(100), base)];
        let closed = close_delivered(
            &mut pending,
            &[("block:x", Some(WriteSeq::from_i64(999)))],
            base + Duration::from_millis(5),
        );
        assert!(closed.is_empty(), "no exact match ⇒ no measurement");
        assert_eq!(pending.len(), 1, "the stale entry is left untouched");
    }

    /// Tokenless ops (toggle/split/delete) correlate by newest-per-target and
    /// drop older same-target entries — a tokenless no-op cannot steal either.
    #[test]
    fn tokenless_closes_newest_and_drops_older() {
        let base = Instant::now();
        let mut pending = vec![
            pend("toggle_state", "block:y", None, base),
            pend(
                "toggle_state",
                "block:y",
                None,
                base + Duration::from_secs(5),
            ),
        ];
        let now = base + Duration::from_secs(5) + Duration::from_millis(7);
        let closed = close_delivered(&mut pending, &[("block:y", None)], now);
        assert_eq!(closed.len(), 1);
        assert_eq!(
            closed[0].ms, 7,
            "newest entry's elapsed, not the older one's"
        );
        assert!(
            pending.is_empty(),
            "older same-target entry dropped as superseded"
        );
    }

    /// Exact match leaves a NEWER same-target entry pending for its own delta.
    #[test]
    fn exact_match_preserves_newer_entry() {
        let base = Instant::now();
        let mut pending = vec![
            pend("set_field", "block:z", Some(50), base),
            pend(
                "set_field",
                "block:z",
                Some(51),
                base + Duration::from_secs(2),
            ),
        ];
        let closed = close_delivered(
            &mut pending,
            &[("block:z", Some(WriteSeq::from_i64(50)))],
            base + Duration::from_millis(3),
        );
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].ms, 3);
        assert_eq!(
            pending.len(),
            1,
            "the newer (seq 51) entry still awaits its delta"
        );
        assert_eq!(pending[0].seq, Some(WriteSeq::from_i64(51)));
    }

    /// End-to-end through the process-global registry: parallel-safe because it
    /// asserts only on its own target.
    #[test]
    fn global_dispatch_and_delivery_clears_entry() {
        interaction_dispatched("toggle_state", "block:e2e-test-a", None);
        rows_delivered("block", [("block:other", None), ("block:e2e-test-a", None)]);
        let pending = PENDING.lock().unwrap();
        assert!(
            pending.iter().all(|p| p.target != "block:e2e-test-a"),
            "matched entry must be consumed"
        );
    }

    #[test]
    fn global_unmatched_ids_leave_entry_pending() {
        interaction_dispatched("set_field", "block:e2e-test-b", None);
        rows_delivered("block", [("block:unrelated", None)]);
        let pending = PENDING.lock().unwrap();
        assert!(pending.iter().any(|p| p.target == "block:e2e-test-b"));
    }
}
