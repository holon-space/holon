# History & Provenance Queries (C2b)

The `block_history` relation (VisionGapAnalysis C2b, ADR 0024 P8) is the
append-only op/effect stream: every op the engine runs, in order, with
provenance (origin, firing transition, driving agent session/tool-call, effect
id). It is a **disclosed ephemeral cache** — Turso-projected, Layer-3,
rebuildable, never authoritative (see `crates/holon-api/src/history.rs` module
docs). Schema: `crates/holon-turso/sql/schema/history.sql`.

**Raw SQL over `block_history` is sanctioned** (Martin's ruling 2026-07-11): the
typed `HistoryStore` accessor is a convenience surface, not an indirection wall.
The `execute_raw_sql` / `execute_query` MCP tools may query the relation and its
joins directly, exactly like the canonical pack below.

## Canonical query pack

Each file lives in `assets/queries/` and is exercised by
`crates/holon/tests/history_query_pack.rs`.

| Query | File | Shape |
|-------|------|-------|
| Q1 supervision | `assets/queries/history_supervision.sql` | Per-session / per-tool-call op counts (`ops` = distinct op groups, `events` = field-delta rows). |
| Q2 transitions | `assets/queries/history_transitions_by_transition.sql` | Op fire counts grouped by `transition_id` — the "postponed N times" primitive generalized. |
| Q3 automations journal | `assets/queries/history_automations_journal.sql` | The user-facing "Daily journal — created 2026-07-10 ⚙" read, over the IVM-maintained `automations_journal` matview (grouped by origin/transition_id/day, `AutomationsJournalSchemaModule`) — not raw `block_history`. |
| Q4 trust × fires | `assets/queries/history_trust_fires.sql` | `TRUST_PROPOSAL_STATS_SQL` acceptance stats per `(origin, transition_id)` LEFT JOINed with history fire counts ("proposed vs did"). |
| Q5 forensic timeline | `assets/queries/history_block_timeline.sql` | Full ordered history for one block (named param `$block_id`). |

## `query_history` MCP tool

The thin `query_history` tool (frontends/mcp/src/tools.rs; worker parity in
frontends/holon-worker/src/lib.rs `dispatch_mcp_tool`) mirrors
`holon_api::HistoryQuery` as its filter, plus a `count` option:

- Filter fields: `entity_name`, `block_id`, `origin`, `session_id`, `field`,
  `new_value`, `day`, `op_group`, `since_millis`, `until_millis`.
- `count: true` returns the match count instead of the event rows.
- **Parse-don't-validate:** the incoming filter is parsed into
  `holon_api::HistoryQueryArgs` (`deny_unknown_fields`) at the boundary — an
  unknown/misspelled key is a loud error, never a silently-ignored filter. It
  projects into `HistoryQuery` and runs through `HolonService::query_history` /
  `count_history`.

Raw SQL over `block_history` via `execute_raw_sql` / `execute_query` is equally
sanctioned for anything the typed filter does not express (joins, aggregates —
the pack above).
