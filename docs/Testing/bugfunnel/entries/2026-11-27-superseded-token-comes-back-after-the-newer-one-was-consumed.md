---
id: 2026-11-27-superseded-token-comes-back-after-the-newer-one-was-consumed
date: 2026-11-27
gap: COVERAGE
status: FIXED
summary: >-
  A failed take put its token back into a single-token row that looked empty
  only because a later take held, or had consumed, a newer token; the old
  token reappeared after the newer one.
---

## Bug
Found by an agent while reasoning about the reader-isolation edges.
Explicit case:
- `T0` takes `k1` from the single-token `P0[s]`; `k2` is written.
- `T2` takes `k2`; the slot is empty again.
- `T0` fails: `k1` goes back into the empty slot, with no version change.
- `T2` commits: the slot is not empty, so its commit moves no version.

`P0[s]` now holds `k1`, older than the consumed `k2`. A reader that read
`k2` commits as not stale on a row that holds `k1`.

## Root cause
- `Tokens::restore` decided "superseded" by slot emptiness only. A newer
  token held by a later take, or already consumed, left the slot empty.

## Missing piece
- Coverage: the executor PBTs drew one taker of a row; no op order had a
  second take of the newer token before the first take failed.

## Remedy
- One function, `Executor::superseded`, decides it for the restore and for
  the reader edges: a single-token row whose version moved since the take
  got a newer token, so the failed take drops its token.
- The isolation PBT draws an optional second taker `T2`, fired by hand.

## Litmus
- COVERAGE, "Was the state never reached?": yes. With `T2` in the drawn
  net, the old rule is red on both op orders.
