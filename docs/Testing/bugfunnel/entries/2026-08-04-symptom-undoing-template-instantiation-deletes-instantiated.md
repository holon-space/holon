---
id: 2026-08-04-symptom-undoing-template-instantiation-deletes-instantiated
date: 2026-08-04
gap: ORACLE
secondary: COVERAGE
status: OPEN
summary: >-
  SYMPTOM: undoing a template instantiation deletes ONE of the 20 instantiated
  children and then reports the undo stack exhausted, stranding the user in a
  partial instance.
source_line: 779
---

## Bug

(dogfood, template feature — item 5 of Martin's list, found while verifying
a successful instantiation) **SYMPTOM: undoing a template instantiation
deletes ONE of the 20 instantiated children and then reports the undo stack
exhausted, stranding the user in a partial instance.** Observed:
`block.instantiate_template {template_id, target_parent, context_key}` on a
20-child template correctly created the whole subtree (20 direct children +
nested grandchildren, task states, bold/link marks and `instance_of` all
preserved, correctly written back to the day page's org file on disk). One
`undo` → `{"success": true, "message": "Operation undone successfully"}`,
child count 20 → 19. A second `undo` → `{"success": false, "message":
"Nothing to undo"}`, count still 19. Net effect: a single logical user
action leaves a state neither the user nor the system asked for, with no
route back to either the clean or the complete state, and the affordance
reporting success. **ROOT CAUSE UNRESOLVED — two competing mechanisms,
neither eliminated.** (a) *Per-write undo granularity*: the instantiation
expands into ~40 individual `create` dispatches, each logging "inverse
operation available", so the stack may simply hold one record per write and
the rest may have been trimmed. (b) *Origin/session-scoped undo filtering*:
every create in this run carried `origin: "agent"` and `session_id:
mcp-session:…`, so the stack may be scoped by origin and the MCP-issued
writes may not be enumerable as user-undoable at all — which would make
"Nothing to undo" a scoping artifact of driving via MCP rather than a
granularity bug, and would mean a human clicking undo in the UI might behave
differently. EVIDENCE LIMITS, per adversarial verification: the first undo
IS corroborated (the retained app log holds exactly one post-instantiation
`op=delete`, 07:38:20.142695Z, succeeded), but the `"Nothing to undo"`
response is a tool response body, never written to the log, so the
load-bearing half rests on the lane's transcript alone. Also note
`crates/holon-api/src/template_instantiation.rs` contains NO undo/inverse
logic — it is a pure planner emitting `plan.creates`; the dispatcher mints
inverses — so it is the wrong file to look at first.

## Missing piece

Whichever mechanism it is, the escape is the same: the template transition
exists in the keystone
(`crates/holon-integration-tests/src/pbt/transitions/instantiate_template.rs`)
and the capability is wired (`SutTemplateInstantiate`), so the interaction
IS generatable, but no invariant relates an undo to the operation that
produced it. Missing piece = an invariant of the form "after `undo`, model
state equals the state before the LAST logical operation", applied to
compound operations, plus a keystone sequence `instantiate_template → undo`
asserting the subtree is fully gone and `redo` restores it fully. Existing
undo coverage only exercises single-write ops, where per-write and
per-operation granularity are indistinguishable — which is why this escaped
regardless of which mechanism is at fault.

## Remedy

OPEN 2026-08-04 — **NEEDS A RE-DRIVE BEFORE ANY FIX LANE.** The
discriminating experiment is cheap: instantiate a template, then undo via
the UI keybinding (user origin) rather than the MCP `undo` tool, and count
children after each step; if the UI path undoes the whole subtree, the bug
is origin-scoping (b), not granularity (a). Only after that should a fix
direction be chosen — do NOT start from "make `instantiate_template` push
one undo record", which presumes (a).
