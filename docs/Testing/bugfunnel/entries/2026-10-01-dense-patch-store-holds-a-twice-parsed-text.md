---
id: 2026-10-01-dense-patch-store-holds-a-twice-parsed-text
date: 2026-10-01
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  dense_patch admitted a body like `x =====`, 100 `/` marks or 99 `*`, whose
  store text did not read back (the writes landed and were taken back),
  because the dispatcher parsed dense_patch's already-parsed content again.
---

## Bug
Found by the Inc 6 round-5 verifier (`lane-logs/inc6rb5v-verify.md`), lane
decision Inc 6. Contract: an edit applies exactly (store and file) or is
refused by row name before any write. These edits passed every planner check,
landed, and verify-after-apply rolled them back. Same class as FIXED
`2026-10-01-dense-patch-admits-edits-it-then-rolls-back`: the planner admits
text that does not read back.

## Root cause
- `parse_dense("* Plan {#0}\nx =====")` gives content `Plan\nx ===` plus a
  verbatim mark; its dense render is `x =====`, so the planner's checks pass.
- `PatchOp::SetContent` went through `set_field(content)` as a String, and
  `OperationDispatcher` (crates/holon/src/api/operation_dispatcher.rs, content
  arm and create arm) treats a String as org source and runs
  `extract_inline_marks_with` on it again: `x ===` became `x =` + verbatim.
  The planner modelled the store as the parse; the store held the parse
  parsed twice. Probe: `lane-logs/inc6r6-rb6-probe1.log`.
- Also `parse_dense` cut the `{#alias}` token and tags after mark extraction
  without shifting the marks (crates/holon-org-format/src/dense.rs
  `marks_after_cut`).

## Missing piece
The generators drew no mark-run body lines (`=`/`/`/`*`/`+`/`~`/`_` runs), so
the engine PBT never produced a text the second parse changes; the planner
had no prediction of the store to check against.

## Remedy
dense_patch sends content as `RowContent {text, marks}` (create: `marks`
set, edit: rich Object), which both providers store as given. The predicate
`refuse_rows_not_written_exactly` (frontends/mcp/src/dense_patch.rs) applies
the plan's ops to the stored rows, projects them again, and refuses by row
name a row whose lines or stars differ; `file_refusals`
(frontends/mcp/src/tools.rs) renders the predicted documents as write-back
does and refuses a row the file would not hold. Generator `mark_run_line`
(crates/holon-integration-tests/src/pbt/dense_text.rs). Red
`lane-logs/inc6r6-rb6-red.log`; green `lane-logs/inc6r6-rb6-green1.log`,
`lane-logs/inc6r6-rb6c-green2.log`; teeth
`lane-logs/inc6r6-rb6c-content-teeth.log`,
`lane-logs/inc6r6-rb6c-content-teeth2.log`,
`lane-logs/inc6r6-rb6c-text-teeth.log`,
`lane-logs/inc6r6-rb6c-stars-teeth.log`.
