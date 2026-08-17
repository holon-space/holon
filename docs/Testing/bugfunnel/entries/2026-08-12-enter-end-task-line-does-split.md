---
id: 2026-08-12-enter-end-task-line-does-split
date: 2026-08-12
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  Enter at the end of a task line does not split the block.
source_line: 720
---

## Bug

(split-ordering lane, task #31, routed as task #33; found by an agent
building a red-first repro, not by a test verdict) **Enter at the end of a
task line does not split the block.** The editor opens on the block's source
projection, so on a tasked block the caret is an offset into `TODO milk` (9)
while `split_block` cuts the content column `milk` (4): prod returns `Split
position 9 exceeds content length 4` and the gesture silently does nothing.
Deterministic 4-transition repro on wiring {Turso}: `CreateBlockUnderFocus
"milk"`, `ToggleState TODO`, `FocusEditableText`, `PressKey[enter]`.

## Root cause

split-ordering lane task #31 (routed as task #33), found by an agent
building a red-first repro — no test verdict named it: **pressing Enter at
the end of a task line does nothing, because prod refuses to split it.** The
editor opens on the block's SOURCE PROJECTION (`focus_editable_text.rs:205`,
caret = `editor_surface_text().len()`), so on a tasked block the caret is an
offset into `TODO milk` (9 bytes) while `BlockOperations::split_block` cuts
the CONTENT column `milk` (4); prod returns `Split position 9 exceeds
content length 4` and the gesture is a silent no-op. Deterministic
4-transition repro, wiring {Turso}: `CreateBlockUnderFocus "milk"` then
`ToggleState TODO` then `FocusEditableText` then `PressKey[enter]`
(`lane-logs/split-red-p2.log` in the lane-split-ordering workspace). ORACLE
by the skill's litmus, taken in order. Coverage: NO — the interaction is
fully generatable, all four transitions are live keystone transitions and
{Turso} is a drawable wiring. Environment: NO — the failing code path
executes in the keystone's own wiring, the op runs and returns its Err
in-test exactly as in prod. Perception: NO — nothing visual. What is missing
is an INVARIANT, and the reason is structural: the oracle is DIFFERENTIAL,
and as of task #31 the reference mirrors prod's refusal BY DESIGN
(`split_block_apply_to_ref` models the Err as a no-op so the ref does not
crash where prod refuses), so reference and SUT now agree on the same wrong
behaviour and every invariant stays green. The only thing that currently
reddens is a driver fail-loud (`driver_input.rs:565` propagating the Err) —
a transport guard, not an oracle, and one
`is_page_boundary_outdent_refusal`-style allow-list entry away from silence.
Missing piece: a NON-differential invariant — a structural gesture the
reference model deems legal must not be refused by the engine — and/or an
offset-space invariant pinning that an editor caret is a SURFACE offset
while `split_block` consumes a CONTENT offset (task #93 territory). NOT
fixed by task #31, which only stopped the reference from asserting where
prod refuses.)

## Missing piece

The interaction is generatable and the failing path runs in-test, so neither
coverage nor environment is the gap. The oracle is differential and the
reference now mirrors prod's refusal by design (task #31's backstop), so ref
and SUT agree on the same wrong behaviour and stay green; the only red is a
driver fail-loud on the propagated Err, one allow-list entry from silence.
Missing: a non-differential invariant (a gesture the reference deems legal
must not be engine-refused) and/or an offset-space invariant separating
SURFACE carets from CONTENT offsets.

## Remedy

OPEN — routed as task #33. Task #31 fixed only the reference-side assert (it
no longer panics where prod refuses); prod behaviour is unchanged. Repro log
`lane-logs/split-red-p2.log`.
