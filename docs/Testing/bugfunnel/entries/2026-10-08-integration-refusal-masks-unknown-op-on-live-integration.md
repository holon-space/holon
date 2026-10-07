---
id: 2026-10-08-integration-refusal-masks-unknown-op-on-live-integration
date: 2026-10-08
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  An unknown operation on a connected, syncing integration was refused as
  "belongs to integration … status: Syncing" at warn level, instead of the loud
  "No provider registered" wiring error.
---

## Bug
Found by the adversarial verifier of the D109 IntegrationSupervisor change
(lane-logs/inc3-verify.md, D1). A probe dispatched `no_such_op_probe` on the
fake integration's entity after it had connected and read `Syncing`, and got
`No provider serves fk-fake-probe.no_such_op_probe: 'fk-fake-probe' belongs to
integration 'fake-mcp' (Fake MCP), status: Syncing` — the error blamed a
healthy integration's state and skipped the `error!` that lists the available
entities.

## Root cause
`OperationDispatcher::integration_refusal`
(crates/holon/src/api/operation_dispatcher.rs) answered for every integration
that owns the entity, whatever its status. The dispatch path takes the
refusal exit before the loud no-provider error, so the refusal meant for a
not-yet-connected integration also covered a live one whose operations are
already in the dispatcher.

## Missing piece
No test or keystone transition dispatches an operation that a live
integration does not offer, so the refusal's status scope was never
exercised; `inv-integration-op-routable-or-refused` only checked the refusal
named the provider, not its status.

## Remedy
`IntegrationStatus::serves_operations()`
(crates/holon-core/src/integration_attribution.rs) is true for Connected,
Syncing and SyncFailing; the refusal applies only when it is false. The
frontend test `an_integration_that_connects_after_the_session_resolved_comes_alive`
now dispatches an unknown operation after the integration is live and asserts
the "No provider registered for entity" error without the integration's name
(red before the fix, green after, red again with the status gate reverted).
The keystone invariant now also requires `status: Connecting` in the refusal
for a peer that has not answered.
