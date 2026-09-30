---
id: 2026-10-01-the-tui-log-grows-without-bound
date: 2026-10-01
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  tui.log (and any HOLON_LOG=file:// target) opens append-only and is never
  truncated or rotated, so it grows for the life of the HOME.
---

## Bug
Found by the verifier of the dogfood follow-ups lane, round 3
(`lane-logs/dffu-r3-verify.md`, Defect 2). A refused second TUI truncated the
running instance's log (39210 to 587 bytes), and a relaunch erased a crash log
with its PANIC line.

## Root cause
The log was truncated at open, before `SessionVault::acquire` refused a second
instance. The truncation is removed; the log now opens append-only. The
consequence is that nothing bounds its size.

## Missing piece
No rotation or size cap. No test checks log growth.

## Remedy
Open, not fixed: rotation is not built.
