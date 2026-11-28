---
id: 2026-11-28-held-reader-stays-held-after-a-write-makes-it-stale
date: 2026-11-28
gap: ORACLE
status: FIXED
summary: >-
  A reader that waited on an open take stayed held for ever when a write
  made its read stale after the hold, and its wait on the parked take
  stayed disclosed.
---

## Bug
Found by the TLA+ spec of reader isolation (TLC trace `StaleReleased`
against the spec mutant that checks stale only at record). Explicit case:
- A write puts `k1` into `P0[s]`; `T0` takes `k1`.
- A write into `P1[s]` fires `T1`, which reads the hole of `P0`: an edge
  to `T0`.
- `T1`'s effect ends Done; it is not stale and has an open edge, so it is
  held.
- A write puts `k2` into `P0[s]`: `T1`'s read is stale.
- `T0` ends Unknown twice and parks: the Condition "`T1` waits for `T0`"
  is raised, and `T1` stays held until someone resolves `T0`.

## Root cause
- `release_held` released a held reader only when it had no edges left;
  the stale test ran only in `record`, before the hold.

## Missing piece
- Oracle: the executor and the model shared the release rule, so the
  differential check agreed on the held reader; no property said that a
  held reader is never stale.

## Remedy
- `release_held` also releases a held reader that the stale test of
  `record` finds stale; its edges go with it, and the Condition falls out
  of `derive_parked_waits`. The model mirrors it.
- Driver property: no held reader is stale after a quiesce
  (`crates/holon-net/tests/exec_support/mod.rs`).
- Static examples `a_reader_stale_by_a_write_after_its_hold_is_released`
  and `..._while_the_take_runs` in `crates/holon-net/tests/exec_isolation.rs`.

## Litmus
- ORACLE, "Was the state reached but judged fine?": the isolation machine
  draws a write into a read row while its reader is held; both sides
  agreed to keep it held.
