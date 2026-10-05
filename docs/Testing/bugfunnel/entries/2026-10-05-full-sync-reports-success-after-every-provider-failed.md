---
id: 2026-10-05-full-sync-reports-success-after-every-provider-failed
date: 2026-10-05
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  `*::full_sync` cleared every sync token, lost both legs on every provider it
  fanned out to, and returned success — while `*::sync` called the same outcome
  an error.
---

## Bug

Found by a verifier on the `wildcard-gate` lane (probe P4,
`lane-logs/wildcard-gate-verify.md` § Defects D1), not by an automated test.
Two providers failing both `clear_cache` and `sync`:

```
P4 is_ok=true trace=["TOKENS CLEARED", "alpha.sync::clear_cache", "bravo.sync::clear_cache",
                     "alpha.sync::sync", "bravo.sync::sync"]
```

The caller asked for a full re-sync, got none of it, and was told it happened.
The sync tokens were cleared on the way, so the next incremental sync starts
from the beginning as well.

## Root cause

`OperationDispatcher::full_sync` (`crates/holon/src/api/operation_dispatcher.rs`)
built a `FanOut` per leg, logged `cleared N/M caches, synced N/M providers`, and
then discarded `fan.failed` — the count reached the log and nothing reached the
caller. `broadcast_sync`, the other broadcast over the same `FanOut`, errored on
exactly that outcome ("`*::sync` failed on every provider that has one"), so the
two broadcasts disagreed about one outcome. Pre-existing on `main`; the lane's
gate work made it visible, not reachable.

A leg that loses SOME providers is a different case — one unreachable external
system must not stop the others from syncing — but that loss was discarded too,
leaving only the log.

## Missing piece

No case can reach the state: the keystone's `FullSync` transition drives
`*::full_sync` through the MCP tool, and the keystone's one syncing provider (the
org provider) does not fail, so there is no fault injection on a fan-out leg
(COVERAGE). Had a case reached it, nothing would have gone red either:
`FullSync::apply_to_ref` records that the state does not change, and no invariant
says a broadcast that reports success actually ran (ORACLE).

## Remedy

`crates/holon/src/api/operation_dispatcher.rs`:

- `FanOut::lost_every_provider` is the rule, read by both broadcasts: a leg
  whose every member failed did not happen and is an `Err` naming the providers
  lost and what already ran (the sync-token clear, and the `clear_cache` leg's
  own outcome).
- `FanOut::summary` names the providers that ran and the ones lost. A partial
  loss keeps succeeding — the fan-out's standing policy — and that summary now
  travels to the caller in the `OperationResult`'s response, not only to the
  log.

Tests (`crates/holon/src/api/operation_dispatcher.rs`):
`a_full_sync_that_lost_every_provider_fails_loudly`,
`a_broadcast_that_lost_one_provider_discloses_it_and_succeeds`.

Red logs: `lane-logs/r3-red-01.log` and `lane-logs/r3-red-02.log` — the first
asserts on `OperationResult { ... response: None }` returned from a full_sync
that lost every provider; the second on `response: None` after one provider was
lost. Green: `lane-logs/r3-green-01.log` (29 dispatcher tests, 0 failed).

Keystone repro: not attempted. Closing the COVERAGE half means a fan-out leg
whose provider can be made to fail from the transition catalog; that generator
extension is **open**, together with the one the sibling entry
`2026-10-05-wildcard-entity-dispatches-any-op-past-every-write-gate` leaves
open.
