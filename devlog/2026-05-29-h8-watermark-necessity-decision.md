# H8 — Watermark necessity (decision gate for Phase 10)

**Date:** 2026-05-29
**Question (plan H8 / ADR 0004 inline comment, lines 49–51):** "Let's discuss
the watermark and if it is really necessary again… When is a watermark strictly
necessary, when can which alternatives be used?" Project memory shows the
quiescence/watermark definition rewritten repeatedly as a chronic race source
(KF-5: CDC-quiescence `[("all_blocks",1)]` benign settling tail). Before Phase 10
builds the Loro→Turso bridge round-trip invariant *on* quiescence, decide:
keep the watermark, or replace it with frontier-equality.

**Verdict: the watermark is strictly necessary for exactly ONE of the four
quiescence components — the Turso CDC/IVM segment — because that is the only
segment with no exposed "applied" frontier to wait on. Everywhere a frontier or
a queue length exists, exact equality is deterministic and strictly better;
replace the watermark there.** Read-only analysis; no code/ADR changed (the ADR
edit is Phase 10's to make — proposed text below).

## The two mechanisms that exist today

1. **Frontier-equality — precise, edge-triggered, deterministic.**
   `wait_for_loro_quiescence_on` (`test_environment.rs:2210`) returns the instant
   `handle.last_synced_frontiers() == doc.oplog_frontiers()`. No budget, no
   churn heuristic — it is a definite equality. This is the *source-side* "caught
   up" signal. The LiveData<Block> feed (the ADR-0004 bridge realization) has the
   same shape: `wait_until` on a monotonic feed sequence (project memory:
   "LiveData::wait_until + items_changed notify", "wait_for_blocks_in_feed").

2. **Watermark-stability — heuristic, budget-bounded, racy.**
   `assert_cdc_quiescent` (`test_environment.rs:1261`) samples
   `cdc_emitted_watermark()` and uses quiescence-with-budget (quiet_for=150ms,
   budget=2s, catchup_grace=50ms) to decide whether post-target activity
   *settles*. It is a heuristic precisely because **Turso IVM exposes no
   matview-applied frontier** — you cannot wait on "all matviews have absorbed
   commit N"; you can only observe that emission has stopped. This is the entire
   source of the chronic race (KF-5).

## Mapping the ADR-0004 four-part quiescence definition

| Quiescence component (ADR 0004:61-65) | Signal available | Watermark needed? |
|---|---|---|
| No in-flight Turso CDC batches (`cdc_emitted_watermark` stable) | **none precise** — IVM has no applied-frontier | **YES — irreducible** |
| No pending file-watcher events | debounce window (explicit, bounded) | No — wait the debounce, not a stability poll |
| No unflushed Loro ops (`oplog_frontiers == last_synced_frontiers`) | **precise frontier-equality** | No — exact equality |
| No scheduled actor work (queues drained) | **precise** — queue length == 0 | No — check emptiness |

Three of four components have an exact condition. Only the Turso-IVM segment is
genuinely frontier-less, so the watermark's necessity reduces to that one
last-mile segment.

## Decision for the Phase-10 bridge round-trip invariant

The bridge reads Loro, writes Turso, owns only a cursor. Gate its round-trip
invariant `read_via_turso(domain) ≅ read_via_loro(domain)` in two parts,
shrinking the racy surface to its irreducible minimum:

- **(a) Source + bridge progress → frontier-equality.** Expose the bridge cursor
  and gate on `bridge.synced_frontier() == loro.oplog_frontiers()` (equivalently,
  wait the LiveData<Block> feed to the target sequence). Deterministic; replaces
  the watermark for the *bulk* of the pipeline (Loro read + bridge cursor + feed).
- **(b) Turso IVM last mile → tightly-scoped watermark-stability.** Keep
  `cdc_emitted_watermark` stability ONLY for matview propagation after the bridge
  has written. Document KF-5's benign tail as expected here. If a future Turso
  version exposes a matview-applied frontier, delete this part too — the bridge
  invariant would then be 100% frontier-gated.

Net: **don't gate the whole bridge invariant on the heuristic watermark.** Gate
the deterministic majority on frontier-equality; reserve the watermark for the
one segment that has no frontier. This is the smallest racy surface achievable
without an upstream Turso change, and it directly answers the ADR's "is it really
necessary" — *necessary, but only for Turso IVM; everywhere else it is strictly
worse than the frontier-equality we already have.*

## Proposed ADR 0004 edit (apply at Phase 10, replacing the inline comment 49-51)

> **Watermark necessity (resolved, H8 2026-05-29):** A watermark is strictly
> necessary only for the Turso CDC/IVM segment, the sole quiescence component
> with no exposed applied-frontier. The other three components have exact
> conditions — Loro frontier-equality (`oplog_frontiers == last_synced_frontiers`),
> file-watcher debounce-window elapse, and actor-queue emptiness — and MUST use
> those, not a stability poll. The Loro→Turso bridge invariant gates source/bridge
> progress on frontier-equality (LiveData<Block> feed sequence) and reserves
> watermark-stability for the Turso-IVM last mile only. Revisit if Turso exposes
> a matview-applied frontier (then the watermark can be removed entirely).
