---
id: 2026-10-01-a-tui-panic-never-reaches-a-live-terminal
date: 2026-10-01
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  While stderr pointed at tui.log, a holon-tui panic printed nothing on a live
  terminal: `true | holon-tui` exited 101 with an empty screen.
---

## Bug
Found by the round-2 verifier of the dogfood follow-ups lane
(`lane-logs/dffu-r2-verify.md`, DEFECT 2). A regression against 7d2d1aeb,
which had no redirect, so the default panic hook printed on the terminal.

## Root cause
`StderrToLog` pointed fd 2 at tui.log and restored it only when the session
returned. The default panic hook writes to fd 2, so the panic text went to
the log. A release build (`panic = "abort"`) runs no destructor, so a `Drop`
restore could not help there.

## Missing piece
No test made holon-tui panic and looked at the terminal.

## Remedy
While stderr points at the log, `StderrToLog` adds a panic hook that also
writes the panic to the terminal's own descriptor, after the previous hook
has logged it. `a_panic_reaches_the_terminal` runs `true | holon-tui` and is
red when only that terminal write is removed (`lane-logs/dffu-r3-sab.log`,
S2). The redirect now covers the whole run, so the boot's stderr line on a
dead terminal cannot panic either (`a_draw_to_a_dead_terminal_ends_the_tui_through_the_session_shutdown`,
red without the redirect, S3).
