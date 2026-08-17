---
id: 2026-07-21-slash-menu-clean-block-shows-exactly
date: 2026-07-21
gap: COVERAGE
secondary: PERCEPTION
status: UNCLASSIFIED
summary: >-
  Slash-menu on a clean block shows exactly 6 entries (Indent, Outdent, Move
  Up, Move Down, Turn into page, Embed Entity) with NO Delete, though the
  block `delete` op exists and executes correctly via `execute_operation` — it
  simply carries no `MenuExposure::Listed` classification, so the 2026-07-20
  menu==Listed correspondence lock stays green while Delete is absent.
  User-perceived missing feature.
source_line: 1076
---

## Bug

Slash-menu on a clean block shows exactly 6 entries (Indent, Outdent, Move
Up, Move Down, Turn into page, Embed Entity) with NO Delete, though the
block `delete` op exists and executes correctly via `execute_operation` — it
simply carries no `MenuExposure::Listed` classification, so the 2026-07-20
menu==Listed correspondence lock stays green while Delete is absent.
User-perceived missing feature.

## Missing piece

No test/decision asserts the `delete` op is user-reachable from the menu —
it was never classified `Listed`, so the correspondence lock passes with
Delete missing (structurally absent, exactly like the 2026-07-19
`convert_block_to_page` COVERAGE row: op reachable via MCP, absent from the
UI). Remedy: classify `delete` (and audit the other structural ops) into
`MenuExposure::Listed` with a positive assertion that Delete appears in a
clean block's menu.

## Remedy

FIX COMMITTED 2026-07-21 (menu_exposure(listed) on CrudOperations::delete +
dual-authority lock; PR #57 pending cascade-guard ruling/merge).
