---
id: 2026-10-01-dense-patch-engine-refusal-names-no-row
date: 2026-10-01
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  When the engine refused a dense_patch op (removal of an authored
  `:_drawer_order:` line), the error named only the block id, not the row
  alias the agent wrote.
---

## Bug
Found by the Inc 6 rebase verifier (`lane-logs/inc6rb2v-verify.md`, D3), lane
decision Inc 6.

## Root cause
`apply_plan` (`frontends/mcp/src/tools.rs:532`) passed an engine error through
unchanged; only planner refusals carried the `{#alias}` label.

## Missing piece
No test made the engine refuse an op of a plan, so the "refusal names a row"
check of the judge never saw an engine error.

## Remedy
`PatchPlan.labels` names every row an op writes; `apply_plan` prefixes a
failing op's error with `PatchPlan::label_of(op)`. Pinned by
`an_op_the_engine_refuses_is_named_by_its_row` (`frontends/mcp/src/tools.rs`).
With the `AuthoredKey` fix the `_drawer_order` removal now applies exactly
(`a_text_org_writes_otherwise_is_refused_before_any_write`).
