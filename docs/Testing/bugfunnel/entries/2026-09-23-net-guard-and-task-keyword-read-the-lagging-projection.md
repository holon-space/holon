---
id: 2026-09-23-net-guard-and-task-keyword-read-the-lagging-projection
date: 2026-09-23
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  Under Loro authority the net gate located a page the running convert had just
  minted, and a keystroke read the task keyword the previous keystroke had
  cleared, both from the SQL projection that had not caught up: the convert
  failed "destination is not a block the store holds", and the keystroke wrote
  a redundant task_state clear (one more operation, one more undo step).
---

## Bug

The landing gate for the savefix chain (`ffcb5394..82112667`) went red in
`just hand-authored` with two failures
(`/tmp/holon-land-w14-1789580360/land-w26-landinggate-1790148800.log`):

- `echo_loop_block_to_page_child_render_leak_parked`, transition 5/5
  `BlockToPage`: `convert_block_to_page: constituent 'move_block' failed: net
  guard: destination block:cb60abcd-… is not a block the store holds`.
- `hand_authored_keystone_regressions`, case
  `task64-toggle-under-open-editor-loro`: `TypeChars.sql_reads: 33 dedup (raw
  132, …) exceeds expected 27 + tolerance 5 = 32`.

Found by the landing gate, not by dogfooding. Recorded because both are
production read-your-own-write defects of the D148.a class that the 2026-09-19
fix did not reach.

## Root cause

Same seam as `2026-09-19-convert-block-to-page-move-races-loro-sql-projection`:
under Loro authority, `block_raw` is written later by the outbound reconcile,
so a read of it inside one gesture can miss that gesture's own earlier write.

1. **Net gate.** `MoveGuard::destination_format`
   (`crates/holon-app/src/move_guard.rs`) located the destination with
   `BlockHomeAuthority::locate` over `dyn BlockReader`, which is
   `CacheBlockReader::get_block_authoritative`, a `block_raw` point read. The
   destination of each child move in `convert_block_to_page` is the page that
   the compound created one step earlier in Loro.
2. **Task keyword.** `OperationEngine::stored_task_keyword`
   (`crates/holon/src/api/operation_engine.rs`) read `properties` through the
   `UndoStateReader`, also `block_raw`. A `source_text` keystroke that follows a
   demotion then still sees the old keyword, so `run_set_source_text` writes
   `task_state = ""` again and pushes an undo entry for it.

The chain did not add either read. It widened the window both reads lose.
`d0de9ed2` removed `save_doc` from every `LoroBlockOperations` write, so an op
returns sooner. It also put `save_all` at the head of the projection's
`emit_ops`, so SQL reflects a write later. In the case-B timeline the
keystrokes are ~33 ms apart at the tip and 40–60 ms apart at `ffcb5394`
(`lane-logs/triage-B-ops-tip-1.log`, `lane-logs/triage-B-ops-ffcb5394-1.log`).

Measured (all logs are under the lane's `lane-logs/`):

| Measurement | tip `82112667` | `9ae0833e` | `ffcb5394` |
|---|---|---|---|
| A, no lag, net-guard red | 3/9 (1/6 population, 1/2 under load, the gate) | 0/5 | 0/6 |
| A, `HOLON_TEST_PROJECTOR_LAG_MS=600`, budget off | 3/3 red | 3/3 red | 3/3 red |
| B, TypeChars dedup | 31–33, 33 in 3 of 13 runs (includes the gate) | 31–32 | 31 in 13/13, plus 31 in 23/23 historical gate runs |
| B, ops per TypeChars | 9 or 10 | 9 or 10 | always 9 |

The 10th op is the redundant `set_field(task_state, "")`. It supplies one
extra distinct read, which is the `INSERT … RETURNING` of its operation-log
row. The second extra distinct read in the 33 runs is the projection writing
content and properties in one `SELECT "content", "properties",
"property_kinds"`, which happens when a slower pass coalesces two changes. That
second read is benign because it lowers the raw count.

The `rename … .sync.<pid>-<n>.tmp … No such file or directory` error that
follows failure A is teardown noise. The failed test drops its temp vault
while the reconcile pass whose absence caused the miss is still running. The
same error appears right after every panic in
`land-w25-landinggate-1790120356.log`. It never appeared in any of the lane's
A runs whose SUT survived.

## Missing piece

Environment: the keystone settles all projections between transitions, so a
read inside one gesture never meets a lagging projection unless the machine is
loaded. Coverage: the alphabet cannot generate projector lag. Only the
single-test lag-lock binaries can open that window deterministically, and
neither site had such a lock.

## Remedy

Each fix was red-first under a lag lock:

- `MoveGuard::destination_format`: when `locate` misses, it asks the write
  authority (`dyn WriteAuthorityReads`; its absence means SQL is the authority
  and the miss is final). When the authority holds the block, it waits up to
  10 s for the block feed to reflect it, fails loud if the feed does not, and
  then locates again. Lock `tests/move_guard_under_lag.rs`: red
  (`lane-logs/triage-lock-red-1.log`, the gate's exact signature), then green
  3/3 (`triage-lock-green-{1,2,3}.log`). The lagged echo-loop replay went from
  3/3 red to 3/3 green (`triage-A-lag600-nobudget-tip-fixed-*.log`).
- `WriteAuthorityReads::block` is a new point read (Loro doc,
  `SqlWriteAuthority`). `stored_task_keyword` now reads through it. Lock
  `tests/task_keyword_under_lag.rs`: red with 2 task_state writes
  (`triage-kwlock-red-1.log`), then green (`triage-kwlock-green-1.log`). Case B
  at the fixed tip: 8/8 at dedup 30–31, always 9 ops.

Both locks were added to `just projector-lag-lock`. `just hand-authored` at
the fixed tip: 9/9 passed (`triage-hand-authored-fixed-1.log`).
