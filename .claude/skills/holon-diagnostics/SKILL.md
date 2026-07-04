---
name: holon-diagnostics
description: Router for diagnosing performance and resource problems in Holon (something is slow, memory-hungry, CPU-bound, stuck/deadlocked, or a query is slow). Start here when you don't yet know WHICH dimension the problem is — it picks the dimension and points at the right tool with its exact invocation. For correctness/wrong-data problems use holon-live-mcp-debugging instead; to classify a discovered bug use bug-gap-triage.
---

# Holon diagnostics — pick the dimension first

The hard part of a perf/resource investigation is **not** running a tool — it's
knowing *which* tool. "The app is slow" could be interaction latency, a CPU hot
loop, a memory blowup, an async stall, or a slow query, and each needs a
different instrument. This skill routes symptom → dimension → tool. Read the
router, run the one tool it points at, then go deep via the tool's own section
or `DEVELOPMENT.md`.

Two golden rules that override convenience (from CLAUDE.md):
- **Always `tee` before filtering** and redirect noisy output to a file (`> file` / `| tee file`).
- The tools answer *different questions*: on-CPU time (samply) ≠ wait time (chrome-trace / tokio-console). Don't use a CPU profiler to find a stall.

## Router

| Symptom | Dimension | Tool (jump to section) |
|---|---|---|
| "This edit/action feels slow (hundreds of ms – seconds) before it shows" | interaction latency | **Latency** — `just measure-latency` / `stage=e2e` |
| "Boot / a whole run is slow" | wall-clock breakdown | **Wall-clock** — chrome-trace |
| "CPU pegged, a hot loop" | on-CPU hotspots | **CPU** — samply |
| "Memory keeps growing / OOM / huge RSS" | allocations | **Memory** — dhat (`just heap-profile`) |
| "It hangs / deadlocks / nothing progresses" | async stall / task starvation | **Async stall** — tokio-console |
| "A query is slow / a matview behaves oddly" | SQL / IVM | **SQL** — trace + MCP EXPLAIN |
| "Weird behavior, don't know where to start" | triage from logs | **Logs** — normalize → drain3/metrics/pm4py |
| "Wrong data / stale rows / divergence" | *correctness, not perf* | → skill `holon-live-mcp-debugging`; classify with `bug-gap-triage` |

If the router is ambiguous, start with **Logs → normalize** (cheapest first
reach) or **Latency** (if the complaint is "slow"), then branch.

## Latency — interaction → visible
The action-to-visible time, broken into stages. This is the first tool for any
"feels slow" complaint.
- `just measure-latency [N]` runs the headless keystone and prints per-action
  p50/p95/max + per-stage cost + the dominator. Or analyze an existing app log:
  run with `RUST_LOG=holon_latency=debug` then `python3 scripts/measure_latency.py <log>`.
- Stages: `dispatch` and `rows` fire in **every** config; `projection` fires
  under CRDT config; `stage=e2e` is the **true end-to-end prod signal**
  (interaction→visible, id-correlated) and the primary SLO — **p95 < 200ms** is
  the bar; above it is a bug. NOTE `action_total` is **harness-only** (never
  emitted in prod) — use `e2e` / `projection` for live numbers.
- Reads: which action is slow, and which stage ate the budget. If `projection`
  dominates under CRDT config, suspect the full-document O(N) reprojection.

## Wall-clock — where a whole run spends time
- Produce a `tracing-chrome` trace (`trace.json`), then
  `python3 scripts/analyze-chrome-trace.py trace.json`.
- Reads: which spans dominate the wall, and sleep/stall loops (this is how the
  3,213×124ms widget-snapshot loop was found). Good for boot and multi-second
  runs where you want the whole timeline, not one action.

## CPU — on-CPU hotspots
- `samply record --save-only …` (gz Firefox profile), then
  `python3 scripts/analyze-samply-profile.py <profile>`.
- Reads: functions burning CPU (e.g. `interpret` at 718k calls/run). **On-CPU
  only** — it does NOT see wait/settle time (its own docstring warns it hides a
  150ms settle). If the problem is waiting, not computing, use chrome-trace or
  tokio-console instead.

## Memory — allocations (dhat)
- `just heap-profile [blocks]` runs a workload under the dhat global allocator
  and writes `dhat-heap.json`; `scripts/analyze_dhat.sh` summarizes total
  bytes/allocations + top sites (skipping allocator frames to name the real
  caller). For the live app: `cargo run -p holon-gpui --features heap-profile`,
  then Ctrl+C to flush.
- Reads: total lifetime bytes, peak-live, top allocators. Baseline finding: Loro
  tree-state (`get_all_tree_nodes_under`) and Turso IVM DBSP dominate the ingest
  path — a legitimate memory-vs-not discriminator.

## Async stall — task starvation / deadlock (tokio-console)
- `just tokio-console-app` launches the live gpui app with a console-enabled
  runtime; or `just tokio-console-harness [blocks] [hold]` for a headless
  attachable run. Then connect: `tokio-console http://127.0.0.1:6669`
  (install: `cargo install --locked tokio-console`). Requires a build with
  `--cfg tokio_unstable` (the recipes set it).
- Reads: per-task poll-times, tasks that never yield, and the Turso
  DatabaseActor surface — the tool for "it hangs" (e.g. the DatabaseActor
  starvation deadlock: `block_in_place` starving the actor).

## SQL / IVM
- `HOLON_TRACE_SQL=1` on the app, then `python3 scripts/extract-sql-trace.py <log>`
  yields a replayable `.sql` for a Turso/IVM repro.
- Live, via the `holon` MCP: `compile_query` (see the SQL a query compiles to),
  `list_tables` / matview definitions, `execute_raw_sql` (run `EXPLAIN` by hand).
- Reads: the actual SQL and plan behind a slow query or a misbehaving matview.

## Logs — triage from a running/finished session
Point these at `/tmp/holon.log` (or a `tee`'d run log). Cheapest first:
- `python3 scripts/normalize-log.py` — coarse GROUP-BY (16k→~1k lines): "what is
  it doing / why is startup slow". First reach when you have no hypothesis.
- `python3 scripts/analyze-log-metrics.py` — ASCII sparklines of RSS, sync/tx
  durations, event rate + outliers.
- `python3 scripts/analyze-log-drain3.py --show-rare` — template-clusters lines,
  surfaces rare/anomalous patterns.
- `python3 scripts/analyze-log-pm4py.py --case-strategy sync_cycle` —
  process-mining: execution patterns and timing bottlenecks.

## Related modes (not perf)
- **Correctness / wrong data / divergence** → skill `holon-live-mcp-debugging`
  (live DB/Loro/org inspection via the `holon` MCP: `diff_loro_sql`,
  `execute_query`, `describe_ui`). Prefer inspecting live state over adding logs.
- **Inspect variables without log statements** → the `debugger-mcp` skill;
  compile with the `debugger` cargo profile.
- **Classify a bug you found** (so QA investment is data-steered) → `bug-gap-triage`.

Depth for every tool lives in `DEVELOPMENT.md` (§"UI Action Latency",
§"Log Analysis", §"Memory & Async-Stall Profiling").
