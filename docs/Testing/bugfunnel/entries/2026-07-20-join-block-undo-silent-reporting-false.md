---
id: 2026-07-20-join-block-undo-silent-reporting-false
date: 2026-07-20
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  join_block undo silent no-op reporting false "undone successfully"
  (execute_operation path; needs editor-path confirm)
source_line: 1049
---

## Bug

join_block undo silent no-op reporting false "undone successfully"
(execute_operation path; needs editor-path confirm)

## Missing piece

undo-correctness oracle for join; join op on undo stack

## Remedy

CLOSED-AS-DESIGNED 2026-07-21 — dogfood-driver artifact: MCP
execute_operation = OpOrigin::Agent, which by ADR 0024 design does not enter
the human undo stack (push gated on origin.is_user(),
operation_engine.rs:1226). Editor backspace-join dispatches as User via
editor_view.rs:1484 and records undo (source-traced; no automated
User-origin join-undo test yet — noted). Honest-no-op agent-surface gap
tracked in the 2026-07-21 MCP-undo-false-success row
