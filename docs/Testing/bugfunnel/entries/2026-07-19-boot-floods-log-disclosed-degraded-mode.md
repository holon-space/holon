---
id: 2026-07-19-boot-floods-log-disclosed-degraded-mode
date: 2026-07-19
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  Boot floods the log with DISCLOSED degraded-mode computed-field failures on
  the seeded layout (GPUI dogfood): 869× `C4 enrich: computed field eval
  failed ... field=is_page_row/is_task/is_legacy_rule error=Variable not
  found: tags/task_state/source_language` + 217× `entity_profile: profile
  condition failed ... Data type incorrect: () (expecting bool)` for
  `is_page_row && (!is_def_var("role") | | role != "page_title")` in ONE boot.
  Fail-loud-but-degraded (substitutes Null / treats variant as non-match) so
  nothing crashes, but the computed-field/profile expressions reference
  variables (`tags`, `task_state`, `source_language`) that the enrich row does
  not carry — a matview↔computed-field variable-binding gap producing
  pervasive Null substitution on every projected row.
source_line: 1013
---

## Bug

Boot floods the log with DISCLOSED degraded-mode computed-field failures on
the seeded layout (GPUI dogfood): 869× `C4 enrich: computed field eval
failed ... field=is_page_row/is_task/is_legacy_rule error=Variable not
found: tags/task_state/source_language` + 217× `entity_profile: profile
condition failed ... Data type incorrect: () (expecting bool)` for
`is_page_row && (!is_def_var("role") | | role != "page_title")` in ONE boot.
Fail-loud-but-degraded (substitutes Null / treats variant as non-match) so
nothing crashes, but the computed-field/profile expressions reference
variables (`tags`, `task_state`, `source_language`) that the enrich row does
not carry — a matview↔computed-field variable-binding gap producing
pervasive Null substitution on every projected row.

## Missing piece

keystone's projection wiring doesn't feed the C4-enrich computed-field
evaluator the same variable set prod does (or the seeded rows legitimately
lack those vars); add an invariant that no projected row triggers a
disclosed computed-field degrade, and reconcile the enrich variable bindings

## Remedy

OPEN — found GPUI dogfood 2026-07-19; disclosed degraded, no data loss
observed but very noisy + potential wrong is_page_row/is_task rendering
