---
id: 2026-10-12-panicked-integration-connect-leaves-its-row-connecting
date: 2026-10-12
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  A connect task that panicked raised `IntegrationConnectFailed` but left the
  integration row at `Connecting` for the rest of the session, although the
  integration serves nothing.
---
## Bug
Found by the supervisor honeymoon experiment (holon-space/holon-work#167,
finding F2), not by a test. `IntegrationSupervisor::start`
(crates/holon-app/src/mcp_integrations.rs) spawns each connect with
`spawn_disclosing_panic`. Its panic handler only emitted the condition. Every
other connect failure writes `Unavailable` to the row, so the sidebar showed a
dead integration as still connecting.

## Root cause
The panic handler disclosed on the bus but did not record the status. The
status write is async and the handler is sync, so it was left out.

## Missing piece
No test drove a panic through the connect task. The release profile
(`panic = "abort"`) ends the process on such a panic, so the path runs only in
a build that unwinds (the test profile today, release once the abort is
removed).

## Remedy
The panic handler also spawns, under the session shutdown, the
`record_status(Unavailable)` write with the panic as its cause. The panic
still has one catch boundary (`spawn_disclosing_panic`).

Pinned by `mcp_integrations::tests::a_panicked_connect_marks_the_integration_unavailable`
(crates/holon-app/src/mcp_integrations.rs): `start()` connects an introduced
connection whose `${PANICKY_PROBE_ARG}` is read from a keychain whose backend
panics. Red before the fix: "a connect task that panicked serves nothing, yet
the row reads \"Connecting\"".

Residual: a connect task that panics after the session shutdown began trips
the `SessionShutdown::spawn` assertion instead of writing the row.
