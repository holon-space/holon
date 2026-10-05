---
id: 2026-10-05-task-state-clear-lost-across-cut-paste
date: 2026-10-05
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  After a block is pasted into a journal day page and its task state is set
  and then cleared, the SUT holds task_state=None while the reference model
  predicts Some("").
---

## Bug
Found by the `keystone-smoke` shrink while gating a land. The failure is
deterministic and red on `main` 85b2cbd4 and on the land candidate, at the
same step with the same signature. Minimal sequence (wiring {Org, Turso},
actor MCPServer, editor leg Dispatch): `SplitBlock(c1, 0)` →
`PasteBlockCopy(split-0, structural-page → a journal day page)` →
`ToggleState(TODO)` → `ToggleState(Clear)` → `FinishCutPaste(SourceSaved)`.
Signature: `inv-task-state-matches-ref` … `expected task_state=Some("")
(reference), actual task_state=None (SUT)`. On `main` the run can stop earlier
on the known red `toggle-state-sql-read-repeat-budget` unless
`HOLON_PERF_BUDGET=0`.

## Root cause
Unmeasured. Candidates: (a) the SUT clear/adopt path drops the cleared state
where the model keeps `""`; (b) the model predicts `""` where an absent value
is the right answer. No measurement yet ties either to the effect. Possibly
related: `2026-09-28-a-block-pasted-into-or-out-of-a-journal-day-page-is-quarantined-as-a-duplicate-id`.

Evidence: `/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/867ae382-a453-4c15-bc47-5f28c8fdaada/scratchpad/land-w2/triage/`
(`probe.jsonl`, `replay.sh`, `replay-A*.log` on the candidate, `replay-B*.log`
on main's dispatcher); gate log `land-w2/landing2.log:1285`.

## Missing piece
The keystone already generates and detects it (`inv-task-state-matches-ref`).
The gap is the oracle: it is not yet known whether the model or the SUT is
wrong, so the invariant cannot be trusted either way.

## Remedy
Open. Registered as known red `task-state-clear-lost-across-cut-paste` in
`docs/Testing/KeystoneKnownReds.md`. Next: decide the correct post-clear value,
then fix the SUT or the model.
