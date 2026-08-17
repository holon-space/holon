---
id: 2026-07-17-boot-enrich-warn-storm-234-557
date: 2026-07-17
gap: PERCEPTION
secondary: ORACLE
status: OPEN
summary: >-
  Boot C4-enrich WARN storm: 234+ (557 across the session)
  `holon_api::computed` "C4 enrich: computed field eval failed — DISCLOSED
  degraded mode, substituting Null" WARNs during boot, all `field=is_task
  error=Variable not found: task_state`. The `is_task` computed field is
  `task_state != ()` (type_registry.rs:441) — authored assuming an absent
  `task_state` resolves to unit `()` — but Rhai variable resolution in
  `resolve_computed_fields_with_scope` (computed.rs:73) raises a hard
  "Variable not found" for every non-task block (which has no `task_state`
  column in its enrich context), so every non-task row on the boot render path
  logs a WARN and the intended `is_task=false` is lost to Null
source_line: 814
---

## Bug

Boot C4-enrich WARN storm: 234+ (557 across the session)
`holon_api::computed` "C4 enrich: computed field eval failed — DISCLOSED
degraded mode, substituting Null" WARNs during boot, all `field=is_task
error=Variable not found: task_state`. The `is_task` computed field is
`task_state != ()` (type_registry.rs:441) — authored assuming an absent
`task_state` resolves to unit `()` — but Rhai variable resolution in
`resolve_computed_fields_with_scope` (computed.rs:73) raises a hard
"Variable not found" for every non-task block (which has no `task_state`
column in its enrich context), so every non-task row on the boot render path
logs a WARN and the intended `is_task=false` is lost to Null

## Missing piece

no invariant caps enrich-path WARN volume / asserts computed fields authored
over optional schema fields evaluate cleanly on rows lacking those fields;
the keystone enriches too few non-task rows at boot to make the storm
visible

## Remedy

OPEN (documented, not fixed — non-trivial, separate subsystem). Root cause
precisely localized: the enrich scope
(`computed.rs::resolve_computed_fields_with_scope`) is built only from the
row's present context keys; a computed expression referencing a
DECLARED-but-absent schema field (`task_state`) is an undefined Rhai
identifier → eval error → disclosed-Null. Correct fix is a boundary/parse
one — seed the enrich scope with every declared schema field as `()`/Null
when the row omits it, so `task_state != ()` evaluates to `false` instead of
erroring — but it needs the profile schema plumbed into the enrich seat and
must NOT become a blanket "unknown var → Null" (that would re-mask genuine
typos, violating fail-loud). Deferred as its own workstream; the current
disclosed-Null behavior is correct-but-noisy, not data loss.
