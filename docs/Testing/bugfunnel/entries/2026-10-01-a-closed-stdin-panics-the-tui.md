---
id: 2026-10-01-a-closed-stdin-panics-the-tui
date: 2026-10-01
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  `true | holon-tui` panics in crossterm ("reader source not set"); a release
  build aborts without the session shutdown.
---

## Bug
Found by the implementer and the verifier of the dogfood follow-ups lane
(`lane-logs/dffu-r2-verify.md`, closed-pipe probe).

## Root cause
crossterm 0.29 panics at `src/event/read.rs:39` when its event reader has no
source, which is the case with a closed pipe on stdin. The panic reaches the
TUI's main task. In release (`panic = "abort"`) the process aborts, so
`shutdown_session` does not run. Nothing was owed in the probe, so no data
loss was measured.

## Missing piece
No check refuses a non-terminal stdin before the event loop starts.

## Remedy
Open. The panic now shows on the terminal
(`2026-10-01-a-tui-panic-never-reaches-a-live-terminal`), and
`a_panic_reaches_the_terminal` uses it. A refusal before the boot (stdin is
not a terminal) would replace the panic; that test then needs the refusal
text instead.
