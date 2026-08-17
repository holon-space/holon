---
id: 2026-08-09-shipped-computed-field-can-call-entity
date: 2026-08-09
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  A SHIPPED computed field can call an entity-lookup Rhai function that is
  never registered on the eval engine
source_line: 1193
---

## Bug

(task #50 lane, surfaced by task #44's verifier — agent code-review, not a
test run) **A SHIPPED computed field can call an entity-lookup Rhai function
that is never registered on the eval engine** — Rhai resolves calls at eval
time, so the field compiles clean, errors at eval, and lands as `()` in
scope, silently degrading every condition it feeds. Concrete escape:
`block_profile.yaml`'s `todo_states` called `document(document_id)`,
unregistered since ADR 0014 retired the `doc:` scheme; doubly dead
(document_id never enters a block row's scope + folded from the
Full-optimized AST), a landmine that would degrade the instant `document_id`
reappeared.

## Root cause

task #50 lane, surfaced by task #44's verifier (agent code-review, not a
test run): **a SHIPPED computed field can call an entity-lookup Rhai
function that is never registered on the eval engine.** Rhai resolves
functions at CALL time, so the field compiles clean, then errors at eval and
lands as `()` in scope — a silent per-field degrade at WARN that inverts
every condition the field feeds (the same #44 profile-condition-degrade
mechanism). CONCRETE ESCAPE: `assets/default/types/block_profile.yaml`'s
`todo_states` called `document(document_id)`; `document` is NOT in
`LiveEntitySpec::ALL` (only `query_source` and `rule_sibling` are ever
registered by `register_entity_lookups`, wired identically in the Turso arm
`create_live_entities` and the Loro arm `spawn_live_entity_refresh`), and
both `document_id` and the `doc:` scheme were retired by ADR 0014 — so the
call was doubly dead: dormant because `document_id` never enters a block
row's scope (`build_scope` iterates only projected columns;
`block_with_query_source.sql` has no `document_id`) AND folded out of the
Full-optimized eval AST. A latent landmine that would degrade the instant
`document_id` reappeared in scope. WHY IT ESCAPED — ORACLE: a field that
errors to `()` is indistinguishable from a legitimately-`()` field, so no
keystone invariant or render oracle can go red on it; there was no assertion
of the reference⊆registered invariant anywhere. SECONDARY COVERAGE: the
`document` branch is also ungeneratable (document_id never in scope), so no
transition sequence reaches it. REMEDY (parse-don't-validate, fail-loud BOOT
guard, not a render-time WARN): `holon_expr::referenced_functions` (AST walk
collecting identifier-named free calls; compiled via `unoptimized_engine()`
so a call the Full optimizer folds away is STILL seen — the source
referencing it is the defect) +
`holon_profiles::validate_lookups_registered` proves every such call in a
bundled profile's computed fields is a Rhai builtin (`is_def_var`) or a
registered `LiveEntitySpec` lookup, wired into `create_default_registry` so
a missing registration is a LOUD boot `Err` naming profile+field+function.
RED-FIRST: `bundled_profiles_only_call_registered_lookups` reds on the real
shipped `document` (lane-logs/50-profiles-bundled-RED.log) BEFORE the fix;
`validate_flags_an_unregistered_lookup` proves firing on a synthetic
`ghost_lookup`; `validate_accepts_registered_lookups_and_builtins` proves it
is not over-eager. FIX: `todo_states` neutered to `()` — behavior-identical
(it was already always `()` post-ADR-0014) — with a comment naming the
retired lookup; restoring per-document TODO-keyword cycles needs a
registered `document` live entity (separate task). NOVEL: no prior row
covers boot-time-unasserted-lookup-registration; nothing to widen.
GAP-NOT-CLOSED, disclosed: the guard covers BUNDLED profiles at boot;
org-embedded USER profiles are not validated against the fixed lookup set
(they may legitimately call other functions), so a user profile referencing
an unregistered lookup still degrades at WARN.)

## Missing piece

a field erroring to `()` is indistinguishable from a legitimately-`()`
field, so no keystone invariant or render oracle can red on it, and there
was no reference⊆registered assertion anywhere; the `document` branch is
also ungeneratable

## Remedy

FIXED 2026-08-09 (task #50): boot-time `validate_lookups_registered` (via
`holon_expr::referenced_functions` over an UNOPTIMIZED AST) proves every
free call in a bundled profile's computed fields is a Rhai stdlib fn
(`RHAI_STDLIB_FREE_FNS`) or a registered `LiveEntitySpec` lookup, wired into
`create_default_registry` → LOUD boot `Err` naming profile+field+fn;
`todo_states` neutered to `()` (behavior-identical post-ADR-0014). GAP NOT
CLOSED: org-embedded USER profiles are not validated at boot — a user
profile with an unregistered lookup still degrades at WARN.
