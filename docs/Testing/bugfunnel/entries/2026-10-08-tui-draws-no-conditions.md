---
id: 2026-10-08-tui-draws-no-conditions
date: 2026-10-08
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  The TUI draws no condition at all: a degradation raised on the core bus,
  including a previous run's panic, never reaches a TUI user.
---

## Bug
Found by code audit in the boot-always A1+A2 lane (round 4). `TuiModule`
(`frontends/tui/src/di.rs`) installs the panic record and hands the bus to the
session, but nothing in `frontends/tui/src` subscribes to it or renders a
condition.

## Root cause
The TUI has no disclosure surface. ADR 0035 (docs/Architecture/Model.md,
"Conditions") requires every frontend to draw each condition, at least as a
toast.

## Missing piece
No TUI test raises a condition and reads the screen.

## Remedy
Open. Until the TUI draws conditions it never calls
`panic_record::seen_on`, so the records of failed runs stay under
`unshown-panics/` and are shown again on each start.
