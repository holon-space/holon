---
id: 2026-10-31-priority-orders-transitions-that-produce-no-common-place
date: 2026-10-31
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  The net loader sorted all transitions by priority, so priority reordered
  transitions that produce no common place, and a lower-priority producer of a
  place fired in the same round as the higher one.
---

## Bug

Two auto producers of one place `Out`, with disjoint inputs, become enabled in
the same round. Both fire, so the lower-priority producer fills `Out` beside
the higher one. And two transitions that produce no common place fire in
priority order instead of file order.

Found by the tester replay (holon-space/holon-work#727), not by a test.

## Root cause

- `crates/holon-net/src/load.rs:85` (base 884b0588) sorted every transition
  of the file by priority, so priority became a global firing order.
- `crates/holon-net/src/exec.rs:2135-2191` (base 884b0588): a round decides
  and starts every enabled transition before any body runs, so the inhibitor
  on `Out` cannot stop the second producer in the same round. The executor
  had no notion of the producers of a place.

## Missing piece

No test drew a net with several producers of one place at different
priorities, nor transitions at different priorities that produce no common
place. The hand-built nets of the executor tests carry no priority.

## Remedy

`TransitionDef` carries `priority` and `produces`. In a round, a producer
fires only when no producer of a common place that comes first in priority
order (higher priority, or equal priority and declared earlier) fires for the
same subject; the others decide again in the next round. The loader keeps
file order. Pinned by `priority_orders_only_the_producers_of_one_place`
(`crates/holon-net/tests/producer_priority.rs`), red before the fix (log
`target/c869/red.log`).
