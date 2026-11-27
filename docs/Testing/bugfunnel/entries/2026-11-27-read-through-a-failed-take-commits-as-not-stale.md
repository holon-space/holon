---
id: 2026-11-27-read-through-a-failed-take-commits-as-not-stale
date: 2026-11-27
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  A reader that bound while a take held the token of a row it read committed
  as not stale after the take failed and put the token back: the restore moves
  no version, and the stale check compared versions only.
---

## Bug
Found by an agent spike on the executor's stale check (shadow stamp rules
beside today's check). Two explicit cases:
- `T0` takes `P0[s]`; `T1` binds and reads `P0[s]` absent; `T0` fails
  definitely or retryably and the token goes back; `T1` records Done.
- The same with a FIFO row: `T1` reads the next token as the front while `T0`
  holds the front; the front goes back; `T1` records Done.

`T1` decided on a view that never committed.

## Root cause
- `Tokens::restore` puts a failed take's token back with no version change,
  so that a restore is not a write.
- `Executor::stale` compared the version of each read row only. The row read
  through the open take had the same version after the restore.

## Missing piece
- Oracle: no property judged a record against the committed state of the rows
  it read. The executor PBTs compare the executor with a model that had the
  same version-only rule.
- Coverage: the wide caller-input generator drew "bind through an open take,
  the take fails, the reader records" in 34 of 1779 cases.

## Remedy
- At bind, the attempt records the tokens that open takes hold from the rows
  it read (`waits_on`). A rollback that puts such a token back marks the
  reader stale (`stale_row`); `stale` returns that row first.
- The model mirrors the edge and the hook.
- `crates/holon-net/tests/exec_isolation.rs`: a reader beside a taker, drawn
  per case, with property P-restore and the spike's cases as static examples.

## Litmus
- ORACLE, "Was the state reached and the wrong answer not judged?": yes. The
  spike's run reached it with today's tests green.
- COVERAGE, "Is there a transition sequence in the current catalog that
  reaches this state?": yes, but rarely drawn; the new machine draws it in
  most runs.
