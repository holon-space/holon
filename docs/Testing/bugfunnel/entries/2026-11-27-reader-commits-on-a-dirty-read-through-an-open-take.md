---
id: 2026-11-27-reader-commits-on-a-dirty-read-through-an-open-take
date: 2026-11-27
gap: ORACLE
status: FIXED
summary: >-
  A reader that bound while a take held the token of a row it read committed
  before the take settled; when the take then failed and put the token back,
  the reader's commit stood on a state that never committed.
---

## Bug
Found by an agent spike on the executor's stale check. Explicit case:
- `T0` takes `P0[s]` and runs an effect; `T1` binds and reads `P0[s]` absent.
- `T1`'s effect ends Done and `T1` commits at once.
- `T0` fails and the token goes back to `P0[s]`.

`T1`'s commit decided on "absent", which never was the committed state. The
stale check cannot catch it: `T1` committed before the restore.

## Root cause
- `Executor::record` committed every non-stale attempt at once, also while a
  take that hid a token of a row it read was still open.

## Missing piece
- Oracle: no property judged a commit against the committed state of the
  rows it read. The executor PBTs compare the executor with a model that
  committed the same way.

## Remedy
- A reader with an open edge (`waits_on`) is held in `record` and released at
  the top of a quiesce round once its takes committed or failed. A take that
  commits moves the reader's read version along; one that fails makes the
  reader stale.
- A parked take keeps its readers waiting until `resolve`, disclosed by
  `Condition::WaitsOnParkedTake`.
- P1 in `crates/holon-net/tests/exec_support/`: every commit's read view
  equals the model's committed front of each row it read (the row's tokens
  plus the tokens open takes hold from it), on every op of every executor PBT.

## Litmus
- ORACLE, "Was the state reached and the wrong answer not judged?": yes. The
  executor PBTs reached it; with P1 added on the unfixed executor, the
  isolation PBT and two answer PBTs went red on it.
