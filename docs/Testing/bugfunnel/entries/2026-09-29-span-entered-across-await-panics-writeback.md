---
id: 2026-09-29-span-entered-across-await-panics-writeback
date: 2026-09-29
gap: ORACLE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Spans entered with `span.enter()` and held across `.await` stayed current on
  tokio worker threads; under load the tracing-subscriber registry then
  panicked ("tried to clone a span ... that already closed") in an unrelated
  task, and when that task was org write-back, edits silently stopped
  reaching disk.
---

## Bug
Found by the Inc 6 keystone-mcp A/B population
(`just keystone-mcp <port> 16 '*:0,DenseProjectionEdit:100'`, scratchpad
`inc6-ab/report.md`): the app logged a tokio-worker panic at
`tracing-subscriber-0.3.23/src/registry/sharded.rs:317` on main and on the
lane. 4 of 4 lane runs with the panic went red on `inv-org-render-fixed-point`
+ `inv-blocks-match-ref/org`; 3 of 3 without it passed. Two other lanes saw
the same assertion (`just hand-authored`
`ingest-clears-a-removed-priority-sqlonly-arm`, a D229 keystone shrink tail).

## Root cause
Four async sites kept an `Entered` guard alive across `.await`:
`LiveData::subscribe`'s actor (`live_data.subscribe_actor`),
`LiveData::wait_for_quiescent`, both `queryable_cache.ingest_batch*` loops,
and `wait_for_loro_quiescence_on`. tracing 0.1.44's `Entered` is `Send`, so
the compiler allows this. The registry keeps the entered span on the
per-thread stack of the thread that entered it. When the task parks, that span
is current for every other task on the thread. When the task resumes on
another worker, the exit misses the first thread's stack, so the id stays
there. When the span then closes, a contextual `new_span` on the first thread
clones a span whose ref count is 0 and panics. A `debug_assert` at
`sharded.rs:265` then fires on the recycled slot. The panic kills whichever
task opened the span. Nothing observes the `JoinHandle`s held by
`SessionShutdown`, so the org write-back tasks (supervisor, consumer,
file-sync-controller) died with only the panic-hook log line.

Evidence:
- All 17 A/B app logs show events of other modules under the leaked actor
  span, 292 to 1949 per run (for example `holon::api::ui_watcher` and
  `holon::api::operation_dispatcher` under
  `live_data.subscribe_actor{source="block"}:live_data.subscribe_actor{source="document_blocks"}`).
- The same invocation after the fix, 4 runs in the test profile (load
  averages 11 to 134 at the ends of the runs): 4 of 4 passed, with 0 span
  panics, 0 panics of any kind, and 0 events of other modules under
  `live_data.subscribe_actor` or `live_data.wait_for_quiescent`. Before the
  fix, 10 of 15 counted runs had the span panic (one-sided Fisher p = 0.033).
  Logs: `lane-logs/post-{1..4}.driver.log`, scratchpad `span-post/`.
- `crates/holon-api/src/live_data.rs`
  `a_quiescence_wait_that_migrates_threads_does_not_panic_the_next_span_on_the_old_thread`
  reproduces the exact production assertion deterministically with the real
  `wait_for_quiescent`: it parks on one thread and finishes on another, and
  the next span on the first thread panics with "tried to clone a span (Id(1))
  that already closed" (red log `lane-logs/red-api-final.log`).

## Missing piece
- No lint forbade a span guard across `.await`.
- No invariant flagged a span that stayed current after its task parked,
  although the contamination is present in every keystone run.
- The keystone-mcp flavour does not see an app-process panic as a failure;
  only the downstream org invariants went red.
- A session task that panicked disclosed nothing (no `WritebackDegraded`).

## Remedy
- The five sites use `Instrument` / `#[tracing::instrument]` instead of a
  held guard.
- A new root `clippy.toml` sets `await-holding-invalid-types` for
  `tracing::span::Entered` and `EnteredSpan`. Measured teeth: with the
  original `live_data.rs`, `cargo clippy -p holon-api --lib -- -D warnings`
  fails at both sites (`lane-logs/clippy-probe-red.log`).
- Red-first unit tests in `crates/holon-api/src/live_data.rs`: two same-thread
  leak tests and the thread-migration panic test.
- `SessionShutdown::spawn_disclosing_panic` reports a task's panic before it
  unwinds. `spawn_supervised` routes a supervisor panic to its give-up seam
  (red `lane-logs/red-supervisor-panic.log`). The org write-back consumer and
  the file-sync-controller disclose a panic as `WritebackDegraded` through
  `disclose_writeback_down` in `crates/holon-orgmode/src/di.rs`.
- Still open: a keystone invariant that fails when the app process logs a
  panic in the MCP flavour.
