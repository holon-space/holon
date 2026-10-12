---
id: 2026-10-12-integration-row-misses-sync-outcomes-before-subscribe-and-after-success
date: 2026-10-12
gap: COVERAGE
secondary: ENVIRONMENT
status: PARTIAL
summary: >-
  An integration row could read `Syncing` for the whole session (its first
  batch landed before the status pump subscribed), and after one good batch it
  read `Connected` forever (persisting failures never reached `Sync failing`).
---
## Bug
Found by the supervisor honeymoon experiment (holon-space/holon-work#167,
finding F1), not by a test. Two defects in one status path:
- `go_live` writes `Syncing`, then the pump calls `health().subscribe()`. A
  batch that finishes between the two is marked seen in the new receiver, so
  `changed()` never fires for it and the row stays `Syncing`.
- `SyncHealthSignal::record` kept `Healthy` on a failed batch. A revoked
  refresh grant after a first sync therefore read `Connected` for the rest of
  the session.

## Root cause
`spawn_status_from_sync_health` (crates/holon-app/src/mcp_integrations.rs)
only reacted to changes after its subscribe. `record`
(crates/holon-mcp-client/src/mcp_integration.rs) folded failure-after-success
into `Healthy` on purpose, as anti-flap, with no limit on how long.

## Missing piece
The keystone generates no integration sync outcomes, so neither a batch before
subscribe nor a failure after a success is reachable from its catalog. The
dedicated suite test `integration_state_sync_failing.rs` covers only "every
batch fails".

## Remedy
- The pump calls `mark_changed()` on its receiver before the loop, so the
  current health is folded in at subscribe.
- `record` keeps the anti-flap intent but no longer hides a lasting failure
  (hysteresis, ruling HF-F1 (c)): a success reads `Healthy` at once; after a
  success, failures read `Failing` only once they have persisted for
  `SYNC_FAILURE_PERSISTS` (10 s, the sync loop's `max_coalesce` window, so a
  persisting failure has met at least one retry). With no success since boot,
  the first failure reads `Failing`.
- Pinned by `a_batch_that_lands_before_the_status_pump_subscribes_still_reaches_the_row`
  and `the_status_pump_follows_sync_failing_and_its_recovery` (holon-app), and
  `one_success_earns_healthy_and_a_single_later_failure_keeps_it` and
  `failures_persisting_past_the_coalescing_window_after_a_success_read_failing`
  (holon-mcp-client).
- PARTIAL: the keystone still generates no sync outcomes.
