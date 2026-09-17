---
id: 2026-09-17-rank-tasks-dead-and-error-swallowed
date: 2026-09-17
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  The `rank_tasks` MCP tool was dead against any real vault — its SQL projection
  omitted five columns `Block::try_from` requires — and reported only the outer
  "Failed to rank tasks" context, hiding the cause.
---

## Bug

Two defects on one path, found by agent exploration during the
`fix-dangling-requires-blocks` lane.

1. `mcp__holon-live__holon_live__rank_tasks` returns the bare string
   `rank_tasks_failed` with no cause.
2. Underneath, the ranker never worked against a live vault at all: every row
   failed to parse.

## Root cause

**The swallow.** `frontends/mcp/src/tools.rs:1340` built its `ErrorData` from
`e.to_string()` on an `anyhow::Error`. That renders only the OUTERMOST context
("Failed to rank tasks", attached at `crates/holon/src/api/holon_service.rs:462`),
discarding the `Caused by:` chain.

**The dead ranker.** `crates/holon/sql/queries/task_blocks_for_petri.sql`
projected `tags` and `requires` but not `advice_suppressed`, `contributes_to`,
`marks`, `collapsed`, or `widget_only`. `Block::try_from`
(`crates/holon-api/src/block.rs:834`) reads every one of those via
`require_string_array`/`optional_bool`, which bail with "required column ...
absent from row". The first row failed the whole call. Confirmed against the
live vault by running the same SQL through the MCP: the `block` matview does
carry all five columns, so the fix is a projection change, not a schema one.

## Missing piece

The existing petri PBTs call `rank_tasks(blocks)` on already-constructed
`Block`s (`crates/holon/tests/petri_e2e_pbt.rs:1236`), so they never cross the
SQL → `Block::try_from` seam. That seam had no test anywhere, so a projection
missing five required columns was invisible. The swallowed error then removed
the only signal an agent had.

## Remedy

FIXED.

- `task_blocks_for_petri.sql` now projects `marks`, `collapsed`, `widget_only`,
  `advice_suppressed`, and `contributes_to` alongside the existing columns,
  reading the same matview aggregates `CacheBlockReader`'s
  `HYDRATED_BLOCK_COLUMNS` documents as required.
- `frontends/mcp/src/tools.rs:1336` formats the full chain with `format!("{e:#}")`.

Red log: `lane-logs/01-defect2-RED.log` — on the pre-fix projection the test
fails with `rank_tasks: failed to parse block rows / Caused by: block block:t1:
required column 'advice_suppressed' absent from row`. Green:
`lane-logs/02-defect2-GREEN.log`. Test:
`api::backend_engine::tests::rank_tasks_full_path_parses_every_required_column`
runs the full path (SQL → parse → petri) against a seeded task.

## Underlying cause beyond the swallow

The task brief suspected the petri engine might be unfinished (vault block
`petri-rank-mcp-claim-release` is DOING). It is NOT: the WSJF engine itself is
sound and its unit tests pass. The failure was purely the SQL projection plus
the error swallow. With both fixed, the run reaches `petri::rank_tasks`
normally.
