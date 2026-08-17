---
id: 2026-07-19-boot-computed-field-profile-flood-fixed
date: 2026-07-19
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  BOOT COMPUTED-FIELD/PROFILE FLOOD FIXED + buried projection gap SURFACED
  (agent design-spike + fix, SqlOnly boot). The 869×/217× (Martin's vault) /
  30×/3× (fresh vault) boot WARN flood — "Variable not found:
  task_state/tags/source_language" (Seat A, `holon_api::computed`) +
  "condition failed to evaluate … () expecting bool" (Seat B,
  `holon_api::entity_profile`) — was one root cause: computed fields blindly
  eval'd over heterogeneous rows, substituting `Null` on a missing column, and
  that `Null` then type-errored the variant conditions that AND it
  (`is_page_row && …` → `() && …`). Fixed by TYPE-AWARE BINDING: each
  `CompiledExpr` now derives its `required_columns` from the rhai AST at
  compile time (`rhai` `internals` feature; `holon-expr`), and the evaluator
  skips (unbound, no rhai call) any field/condition whose required column is
  ABSENT from scope — silent for optional properties (`task_state`) and
  UI-state vars, LOUD-once for DECLARED columns. After-boot: old flood 0/0/0;
  the ONLY remaining lines are 3 deduped LOUD "DECLARED column absent" naming
  `source_language` from `source="focus_roots"` — the BURIED REAL BUG the
  flood hid: the focus_roots subscription yields block-scheme rows that omit
  declared block columns (`source_language`, and edge-field `tags`), so the
  block profile's computed fields
  (is_holon_source/is_legacy_rule/is_rule_head/is_page_row) cannot bind.
  Baseline `~/.claude/jobs/00b6f50c/tmp/computed-vars-boot-baseline.log` (33
  flood), after `…/computed-vars-boot-after.log` (0 flood, 3 loud).
source_line: 1023
---

## Bug

BOOT COMPUTED-FIELD/PROFILE FLOOD FIXED + buried projection gap SURFACED
(agent design-spike + fix, SqlOnly boot). The 869×/217× (Martin's vault) /
30×/3× (fresh vault) boot WARN flood — "Variable not found:
task_state/tags/source_language" (Seat A, `holon_api::computed`) +
"condition failed to evaluate … () expecting bool" (Seat B,
`holon_api::entity_profile`) — was one root cause: computed fields blindly
eval'd over heterogeneous rows, substituting `Null` on a missing column, and
that `Null` then type-errored the variant conditions that AND it
(`is_page_row && …` → `() && …`). Fixed by TYPE-AWARE BINDING: each
`CompiledExpr` now derives its `required_columns` from the rhai AST at
compile time (`rhai` `internals` feature; `holon-expr`), and the evaluator
skips (unbound, no rhai call) any field/condition whose required column is
ABSENT from scope — silent for optional properties (`task_state`) and
UI-state vars, LOUD-once for DECLARED columns. After-boot: old flood 0/0/0;
the ONLY remaining lines are 3 deduped LOUD "DECLARED column absent" naming
`source_language` from `source="focus_roots"` — the BURIED REAL BUG the
flood hid: the focus_roots subscription yields block-scheme rows that omit
declared block columns (`source_language`, and edge-field `tags`), so the
block profile's computed fields
(is_holon_source/is_legacy_rule/is_rule_head/is_page_row) cannot bind.
Baseline `~/.claude/jobs/00b6f50c/tmp/computed-vars-boot-baseline.log` (33
flood), after `…/computed-vars-boot-after.log` (0 flood, 3 loud).

## Missing piece

The failing path (focus_roots projecting column-poor rows resolved through
the block profile) runs only in the real SqlOnly boot wiring; the headless
keystone enriches well-formed rows, so it never produced a
heterogeneous/column-poor block row and its `inv-no-observed-errors` oracle
keys on ERROR, not the WARN-level disclosed degrades, so the whole flood sat
below the oracle threshold. Now that the signal is a DISCLOSED, deduped
`warn!("DECLARED column absent" context/column)`, an ORACLE becomes
feasible: a boot-time invariant asserting **zero** "DECLARED column absent"
lines (parity of every entity's declared columns against what its display
projection carries). The deeper ENVIRONMENT remedy is to make the
`focus_roots` projection carry the block's declared columns (source_language
+ hydrated tags), so the block profile binds cleanly.

## Remedy

FIXED (flood) 2026-07-19 — type-aware binding landed across `holon-expr`
(`CompiledExpr.required_columns` + AST walk under rhai `internals`, 16
tests), `holon-api` (`computed.rs` classified eval + deduped
`warn_missing_declared_column`; `entity_profile.rs` `eval_condition` +
`EntityProfile.declared_columns` + unbound-as-absent scope semantics, 3 new
tests), `holon-profiles` (`declared_columns` from
`TypeDefinition::persistent_fields()`, per-variant
`condition_required`/`data_condition_required`, 3 new tests). OPEN
(focus_roots projection gap + the boot-parity ORACLE) — flagged to
orchestrator as the follow-up; the LOUD line now makes it observable.
