---
id: 2026-09-01-integration-initial-sync-failure-swallowed
date: 2026-09-01
gap: ORACLE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  An integration's whole initial sync aborts on a SQL datatype mismatch, is
  logged only as a WARN with no user-visible disclosure, and the sidebar keeps
  showing the provider as Connected with an empty view.
---

## Bug

Found by `dogfood-explorer` pass #2 over v0.0.23 (`d49ef0316a77`), reading the
app log after enabling the five bundled integrations.

```
WARN live_data.subscribe_actor{source="block"}: holon_mcp_client::mcp_integration:
  initial sync failed error=Batch transaction failed: Database error:
  Failed to execute statement: datatype mismatch
```

Consequences observed live:

- The sidebar shows the affected provider with the `Connected` status symbol.
- Its authored default view opens and renders no rows.
- `jsonplaceholder` — the one bundled provider that needs no credentials and so
  should have synced — has `SELECT count(*) FROM jp_posts` = **0**.
- Nothing is surfaced in the UI. The only toast shown was gmail's unrelated
  "Integration unavailable" banner.

So the user sees a provider reporting itself connected, with an empty view, and
no indication anywhere that its data load failed. That is the "silently degrades
to look fine" case the repo's error-handling philosophy ranks as never
acceptable.

## Root cause

`crates/holon-mcp-client/src/mcp_integration.rs:1374-1380`:

```rust
if let Err(e) = sync_engine.sync_all().await {
    warn!(error = %e, "initial sync failed");
}
```

`PendingSyncWork::execute` treats a full sync as subsuming every pending
per-URI resync and poll tick, so this single swallowed error aborts the entire
initial data load for the provider. The error is reduced to a `warn!` and
discarded: it is not propagated to the caller, not reflected in
`integration_state.status`, and not disclosed to the user.

The underlying `datatype mismatch` is a second, separate defect — a column type
disagreement between the sidecar's declared entity schema and the values the
batch insert binds — and is not diagnosed here. The swallowing is what makes it
invisible.

Evidence: `/tmp/dogfood2-0901/logs/app3.log` (one occurrence);
`integration_state` and per-provider entity tables queried live over MCP.

## Missing piece

Two distinct absences:

1. No invariant asserts that a provider reporting `Connected` has actually
   completed its initial sync. The status column is derived from live
   connectivity alone, so a total data-load failure cannot move it. The state is
   reachable and observable in SQL, so this is an ORACLE gap.
2. No fail-loud path. The repo rule is explicit — never swallow errors, enrich
   and surface them. A `warn!` in a log the user never reads is a swallow.

The ENVIRONMENT secondary: the keystone wires no MCP-client integrations, so
`sync_all` never runs in the test environment at all.

## Remedy

Open. Proposed:

1. Propagate the sync failure into `integration_state` as a distinct failed
   status with the error text attached, and disclose it the way gmail's
   unavailability is already disclosed (banner/toast naming the provider and
   the cause). A provider whose data never loaded must not read `Connected`.
2. Diagnose the `datatype mismatch` itself: identify which entity's declared
   column types disagree with the bound values, and fail loud at the schema
   boundary instead of at the batch insert (parse, don't validate).
3. Add the invariant from (1) so the keystone can go red once integrations are
   wired into the test environment.
