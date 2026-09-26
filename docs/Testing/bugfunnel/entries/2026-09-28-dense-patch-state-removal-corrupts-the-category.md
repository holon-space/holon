---
id: 2026-09-28-dense-patch-state-removal-corrupts-the-category
date: 2026-09-28
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  dense_patch removing a row's task state writes `task_state = ""` and
  `task_state_category = ""`; the org renderer then panics on the corrupt
  category, so the block's org write-back dies on a runtime worker.
---

## Bug
Found by the Inc 6 round-4 lane in its `update`-op spike
(`lane-logs/inc6r4-spike-setfield.log`, probe P4) on the composed
`full_headless` session. The two `set_field` writes the dense_patch applier
issues for `SetState { task_state: None }` — `task_state = ""`, then
`task_state_category = ""` — left the block with `task_state_category: ""`.
The next org render panicked:
`corrupt task_state_category Some("") on block block:sp-d (expected "active" or "done")`
(`crates/holon-org-format/src/models.rs:794`). Present on main `d4f426ffc965`
(same applier, `frontends/mcp/src/tools.rs:802-819` there).

## Root cause
`LoroBlockOperations::set_field("task_state", …)` already writes the category
sidecar in the same commit, and clears both keys only for `Value::REMOVED`
(`crates/holon-loro/src/loro_block_operations.rs:829-850`). The applier
clears with empty strings and then overwrites the derived category with `""`.

## Missing piece
The planner oracle modelled the store by hand, so "keyword removal" read as
applied exactly (`frontends/mcp/tests/dense_patch_exact.rs` row-text table).
No real-engine rung and no keystone transition removes a state through
dense_patch.

## Remedy
Inc 6 round 4: a state removal writes `task_state = REMOVED`, which clears
both keys in one commit (`PatchOp::SetState` in `frontends/mcp/src/tools.rs`).
`a_state_removal_reads_back_and_reaches_the_file` in
`crates/holon-integration-tests/tests/dense_patch_engine_exact.rs` timed out
on the renderer panic at base (`lane-logs/inc6r4-red.log`) and passes now; the
generated real-engine property draws state removals.
