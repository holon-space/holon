# Live Oracles — keystone invariants in debug builds

Debug builds run a subset of the keystone PBT invariants as **background
assertions against the live app**, so every manual dogfood session is an
oracle-carrying session. Rationale: the quality audit showed manual-testing
escapes are dominated by ENVIRONMENT bugs — prod is their home field, and the
keystone's invariants only ran in tests.

## What runs when

| Piece | Where | Cadence |
|---|---|---|
| Cheap structural tier (`inv-no-orphan-blocks`, `inv-no-parent-cycles`, `inv-source-language-iff-source`) | `holon_oracles::runner` — tokio task spawned by `frontends/gpui/src/main.rs`, queries via `BackendEngine::execute_query` (same read path as the embedded MCP server, off the GPUI thread) | every 2s |
| Latency SLO (`latency-slo`) | `holon_oracles::latency::LatencySloLayer` — a tracing Layer installed by `holon_frontend::logging::init`, watching the existing `holon_latency` stage events | per emitted stage event, zero polling |

Latency stages and where they fire:

- **`e2e` — the PRIMARY SLO signal**: true interaction → visible-to-render
  wall time, in every configuration. Implemented by
  `holon_api::latency_e2e`: `dispatch_intent{,_sync}` registers the op +
  target entity id; the first `LiveData` CDC batch whose rows touch that id
  (as row id, or as `parent_id` of a created row — covers create/split)
  closes the clock and emits `stage="e2e" action=… block=… source=… ms=…`.
  This is the prod counterpart of the harness-only `action_total` stage and
  is consumed by `scripts/measure_latency.py`. An `e2e` event slower than
  **200ms** (`HOLON_ORACLES_SLO_MS` to tune) is a violation: banner + error.
- `dispatch`, `rows` (every config) and `projection` (CRDT only) are
  **diagnostic attribution**: a stage exceeding the budget logs a
  `tracing::warn` (`[latency-slo diagnostic]`) naming the stage that ate
  the budget, but does not raise the banner — the `e2e` stage carries the
  verdict.

Boundary disclosures: final GPU paint is out of scope; ops without an `id`
param and deletes whose CDC id is a rowid are not correlated (no `e2e`
event); pending interactions expire after 30s.

## Surfacing (fail loud, never fake)

A violation is loud in **both** channels:

- **UI banner**: full-width red bar pinned to the top of the GPUI window
  (`frontends/gpui/src/oracles_ui.rs::render_banner`), listing each
  violation. `dismiss` clears sticky latency entries; structural violations
  reappear on the next runner cycle while the data is still broken — they
  cannot be silenced.
- **Log**: `tracing::error!(target: "holon_oracles", ...)` with the full
  message.

A failed oracle *snapshot* (SQL error) is itself reported as a violation —
never silently skipped.

## Tiers and env var

- Debug builds only (`cfg(debug_assertions)`); release builds carry nothing.
- `HOLON_ORACLES=off` — opt out entirely (runner, bridge, latency layer).
- unset / `on` / `cheap` — cheap tier (default ON in debug builds).
- `HOLON_ORACLES=full` — reserved for heavier checks (e.g. org-render
  fixed-point); currently identical to `cheap`.

## Architecture / how to add an oracle

`crates/holon-oracles` is a small prod crate (deps: holon-api, tokio,
tracing; **no proptest, no test-crate dependency** — the prior audit's
"proptest in prod builds" finding stays fixed):

- `checks.rs` — **pure check functions** over minimal typed rows. This is the
  single shared implementation: the keystone PBT bodies
  (`holon-integration-tests/src/pbt/invariants/bodies/`) delegate to these,
  and the live runner feeds them from SQL snapshots. One implementation, no
  drift.
- `runner.rs` — cadence loop + `OracleStateAccess` trait (implemented over
  `BackendEngine` in `frontends/gpui/src/oracles_ui.rs`).
- `status.rs` — process-global violation ledger + watch channel.
- `latency.rs` — the SLO tracing layer.

To add a structural oracle:

1. Write a pure function in `holon_oracles::checks` taking typed rows,
   returning `Vec<String>` violation messages (+ unit tests).
2. Make the keystone body delegate to it (see `no_orphan_blocks.rs` for the
   pattern) — the PBT keeps proving the check itself.
3. Add the snapshot method to `OracleStateAccess` and its SQL impl in
   `oracles_ui.rs` (parse at the boundary, fail loud).
4. Chain it into `runner.rs::run_cheap_cycle` (or gate behind
   `OracleMode::Full` if heavy).

Ref-reading invariants (those whose `Needs.ref_present` is non-empty) can
NOT be lifted — there is no reference model in a running app. See the
classification table in the ws-oracles workstream report.

## Overhead

The runner logs per-cycle timing under
`target="holon_oracles" stage="cycle" ms=… matview_rows=… raw_rows=…
next_sleep_ms=…` (debug level) so overhead at any vault size is directly
measurable from the log. The checks run entirely off the GPUI thread; the
only shared resource is the DB read path.

Measured (2026-07-07, debug build, M-series):

- idle @ 1.1k blocks: **45–50ms/cycle** (~2.5% of one background thread at
  the 2s floor cadence)
- idle @ 8k blocks: **~450ms/cycle** (backoff paces the cadence to ~2.2s)
- under concurrent write load (initial org ingest, live watchers) the same
  cycle takes **2–10s+** — dominated by Turso scan/contention cost, not the
  check algorithm (measured on a loaded instance:
  `SELECT id,parent_id FROM block` = 1.1s, even `SELECT count(*) FROM block`
  = 774ms; the matview read path's scan cost at scale is itself a prod-bug
  candidate)

Because the scan cost grows superlinearly, the runner uses **adaptive
back-off**: it sleeps `5 ×` the previous cycle's duration (floor 2s, cap
60s), bounding the duty cycle to ≤ ~17% of one background thread at any
vault size. Under heavy write load (initial org ingest) a cycle can
transiently take tens of seconds — the back-off stretches the cadence
accordingly.
