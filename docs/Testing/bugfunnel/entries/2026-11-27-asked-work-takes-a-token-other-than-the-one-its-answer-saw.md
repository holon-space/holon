---
id: 2026-11-27-asked-work-takes-a-token-other-than-the-one-its-answer-saw
date: 2026-11-27
gap: COVERAGE
status: FIXED
summary: >-
  With asks reading the committed view, an asked work on a FIFO row took the
  row's head while an open take held the front its answer's situation showed.
---

## Bug
Found by an agent while checking the committed-view ask rule for write skew.
Explicit case:
- `P0[s]` is FIFO. `T0` takes the front `k1` (n = 1) and retries; `k2`
  (n = 2) is written.
- The ask of `W0` reads the committed front `k1`; the user answers on n = 1.
- `W0` pops the row head `k2`. Whether `T0` then fails or commits, no serial
  order explains "W0 took k2 on an answer about k1".
- Also when the answer comes first and a take of `k1` starts in the same
  quiescence, ahead of the work.

## Root cause
- The ask read the committed front; the work's take popped the actual head.

## Missing piece
- Coverage: no executor test drew a FIFO row with an asked work beside
  another taker.

## Remedy
- An asked work takes only the committed front: while an open take holds
  it, the work waits; a parked holder is disclosed until `resolve`.
- Static examples in `crates/holon-net/tests/exec_answer.rs` for both
  orders and the parked holder.

## Litmus
- COVERAGE, "Was the state never reached?": yes. Both static examples are
  red without the wait.
