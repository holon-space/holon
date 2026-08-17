---
id: 2026-08-14-typed-task-keyword-dropped-both-stores
date: 2026-08-14
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  A typed task keyword is dropped by BOTH stores while the reference keeps it
source_line: 705
---

## Bug

(pump-redesign lane D12.b; found by running the keystone under the armed
interleaving axis, no gate config produces it) **A typed task keyword is
dropped by BOTH stores while the reference keeps it** —
`inv-task-state-storage-coherence` reports `ref=Some("TODO") sql=Some("")
loro=Some("")`. Reachable only with `HOLON_PBT_SCHED_KINDS=TypeChars` armed,
where keystrokes go through the fire-and-forget door production uses
(`dispatch_intent`) instead of the awaiting door the default keystone
drives; first panic in 3/3 armed runs and present on a tree with no
schedule-point code, so it predates the pump redesign.

## Root cause

secondary COVERAGE, pump-redesign lane (D12.b), found by RUNNING THE
KEYSTONE UNDER THE ARMED INTERLEAVING AXIS — no gate configuration produces
it: **a typed task keyword is dropped from BOTH stores while the reference
keeps it (`inv-task-state-storage-coherence`: `ref=Some("TODO") sql=Some("")
loro=Some("")`).** Only reachable with `HOLON_PBT_SCHED_KINDS=TypeChars`
armed, i.e. when keystrokes dispatch through the FIRE-AND-FORGET door
production GPUI actually uses (`dispatch_intent`), rather than the awaiting
door (`dispatch_intent_sync`) the default keystone drives; lands ~2 masked
transitions into case 1, first panic in 3/3 armed runs, and reproduces on a
tree with NO schedule-point code
(`lane-logs/pump-redesign/AB-inc0-armed-burst.log`), so it predates the
completion-driven pump. The oracle is NOT the gap —
`inv-task-state-storage-coherence` fires correctly the moment the path runs;
the gap is that the default test wiring never runs prod's real dispatch door
concurrently. Registered in KeystoneKnownReds as
`task-state-storage-coherence`, UNOWNED.)

## Missing piece

the default keystone wiring dispatches keystrokes through
`dispatch_intent_sync`, so prod's concurrent-dispatch path is never
exercised unless the interleaving axis is armed by hand

## Remedy

OPEN — registered as `task-state-storage-coherence` in KeystoneKnownReds.md
(UNOWNED); the completion-driven pump (D12.b) is the parity work that makes
this schedule space reachable, and arming the door in a gate tier is the
remedy
