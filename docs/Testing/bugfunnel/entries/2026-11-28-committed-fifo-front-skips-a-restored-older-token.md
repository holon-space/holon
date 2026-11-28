---
id: 2026-11-28-committed-fifo-front-skips-a-restored-older-token
date: 2026-11-28
gap: ORACLE
status: FIXED
summary: >-
  The committed front of a FIFO row was the oldest token an open take held,
  even when the row itself held an older token that a failed take put back.
---

## Bug
Found by an agent while writing the static example for the FIFO front
order. Explicit case:
- `P0[s]` is FIFO with `k1`, `k2`. `T0` takes `k1`, `T2` takes `k2`.
- `T0` fails: `k1` is back in the row; `T2` still holds `k2`.
- A read of the committed view showed `k2`; the committed row is
  `[k1, k2]`, so its front is `k1`.

## Root cause
- `committed_front` took the oldest held token first and fell back to the
  row's head only when no take held a token of the row.

## Missing piece
- Oracle: the executor and the model shared the rule, so the differential
  check agreed on the wrong front; no property named the committed front.

## Remedy
- The committed front of a FIFO row is the oldest by token id over the
  row's tokens and the held ones, in the executor and the model.
- Static example `committed_front_is_a_restored_token_older_than_a_held_one`
  in `crates/holon-net/tests/exec_isolation.rs`.

## Litmus
- ORACLE, "Was the state reached but judged fine?": the isolation machine
  can draw it with a committed-view reader, and both sides agree on it.
