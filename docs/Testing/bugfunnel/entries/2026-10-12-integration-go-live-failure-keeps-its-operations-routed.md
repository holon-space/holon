---
id: 2026-10-12-integration-go-live-failure-keeps-its-operations-routed
date: 2026-10-12
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  `go_live` registers the connector's operations before its last fallible
  steps; a later failure writes `Unavailable` but leaves the operations routed,
  and the FDW tables, clock grains and matview hook installed.
---
## Bug
Found by the supervisor honeymoon experiment (holon-space/holon-work#167,
finding F3), not by a test. In `IntegrationSupervisor::go_live`
(crates/holon-app/src/mcp_integrations.rs) the order is FDW tables, clock
grains, matview hook, `register_provider`, then the fallible
`derive_write_authority` and `request_initial_sync`. The failure arm writes
`Unavailable`; the dispatcher is append-only, so nothing removes the
operations.

## Root cause
Effects run before checks. The dispatcher has no unregister, by design.

## Missing piece
No test reached a go-live failure after `register_provider`. Through the
production wiring the only one is `request_initial_sync` refused by a sync loop
that has stopped: a panicked batch, which `panic = "abort"` turns into a process
exit today and an unwinding build turns into a dead loop. The loader already
refuses every type `derive_write_authority` would refuse, and cross-sidecar
entity collisions are refused before connect.

## Remedy
`go_live` runs every check before the first effect: `check_provider` on the
connector and on each derived write authority
(`type_declaration::derived_write_authority`, built but not registered), and a
check that the sync loop still runs. Only then does it hold the clock grains,
register the FDW tables and the matview hook, and register the operations.

Pinned by `mcp_integrations::tests::an_integration_that_cannot_go_live_routes_no_operation`
(crates/holon-app/src/mcp_integrations.rs): a real child-process sidecar
connects, its sync loop is stopped, and `go_live` must fail with no operation
of the integration in the dispatcher. Red before the reorder: "going live
failed (enqueuing the initial sync of 'half-live': sync event loop is not
running …), yet the dispatcher routes 'halflive_items'".

Residuals: the entity types stay registered after a failure; a clock grain held
before a later grain fails stays held; a check and its effect are not atomic,
so a sync loop that stops between them still fails after `register_provider`.
