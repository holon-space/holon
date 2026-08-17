---
id: 2026-07-19-slash-menu-convert-block-page-unreachable
date: 2026-07-19
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  SLASH-MENU `Turn into page` (convert_block_to_page) and `Instantiate
  template` op are UNREACHABLE via the GPUI slash menu — DISTINCT root cause
  from the scroll cap above (task 2026-07-19 conflated the two). The engine
  advertises both as SYNTHETIC descriptors appended in
  `OperationEngine::available_operations("block")`
  (`crates/holon/src/api/operation_engine.rs:1244-1249`), so MCP
  `list_operations` and the PBT/verifier see them — but the GPUI editor's
  slash-menu operation list comes from a DIFFERENT source:
  `services.resolve_profile(row).operations`
  (`crates/holon-frontend/src/reactive_view.rs:63` →
  `session.resolve_row_profile` → `ProfileCache::operations_for`), which is
  the profile/dispatcher op set and never includes the two engine-synthetic
  descriptors. Live-confirmed 2026-07-19: MCP `list_operations block` lists
  "Turn into page"+"Instantiate template", but the live menu on a content
  block shows only the 16 profile-satisfiable ops (Indent…Set Field) and
  `/page` filters to zero matches. (Per-template picker entries are a
  separate, working path — `view_event_handler`
  `with_templates(list_templates())` — so real vault TEMPLATES do reach the
  menu, below the fold, and the scroll fix above makes them reachable; only
  the two engine-synthetic OPS are absent.) The knmxwoox "slash-menu
  descriptor; verifier CONFIRMED" almost certainly confirmed via the
  MCP/available_operations path, not the actual GPUI menu.
source_line: 1018
---

## Bug

SLASH-MENU `Turn into page` (convert_block_to_page) and `Instantiate
template` op are UNREACHABLE via the GPUI slash menu — DISTINCT root cause
from the scroll cap above (task 2026-07-19 conflated the two). The engine
advertises both as SYNTHETIC descriptors appended in
`OperationEngine::available_operations("block")`
(`crates/holon/src/api/operation_engine.rs:1244-1249`), so MCP
`list_operations` and the PBT/verifier see them — but the GPUI editor's
slash-menu operation list comes from a DIFFERENT source:
`services.resolve_profile(row).operations`
(`crates/holon-frontend/src/reactive_view.rs:63` →
`session.resolve_row_profile` → `ProfileCache::operations_for`), which is
the profile/dispatcher op set and never includes the two engine-synthetic
descriptors. Live-confirmed 2026-07-19: MCP `list_operations block` lists
"Turn into page"+"Instantiate template", but the live menu on a content
block shows only the 16 profile-satisfiable ops (Indent…Set Field) and
`/page` filters to zero matches. (Per-template picker entries are a
separate, working path — `view_event_handler`
`with_templates(list_templates())` — so real vault TEMPLATES do reach the
menu, below the fold, and the scroll fix above makes them reachable; only
the two engine-synthetic OPS are absent.) The knmxwoox "slash-menu
descriptor; verifier CONFIRMED" almost certainly confirmed via the
MCP/available_operations path, not the actual GPUI menu.

## Missing piece

No test opens the GPUI slash menu and asserts the engine-synthetic ops
(`convert_block_to_page`, `instantiate_template`) are present — the keystone
drives ops via the dispatcher/available_operations path, not the
profile-sourced editor menu, so a descriptor advertised by
`available_operations` but absent from `resolve_profile().operations` is
invisible. The `count(popup items) == count(satisfiable profile ops) +
count(templates)` assertion proposed above would NOT catch this (it counts
profile ops, which is exactly what the menu shows); the assertion must
instead compare menu items against `available_operations` (the engine's
advertised set) to catch a synthetic op that never reaches the frontend.

## Remedy

OPEN — NOT fixed here (out of the assigned scroll+placeholder scope; the fix
is architectural: engine-synthetic descriptors must be threaded into the
frontend menu's operation source, either by adding a dedicated menu path in
`view_event_handler` mirroring `with_templates`, or by having
`resolve_profile`/`ProfileCache` include the synthetic block ops. Needs a
ruling on where synthetic-op injection belongs before implementing). Flagged
to orchestrator.
