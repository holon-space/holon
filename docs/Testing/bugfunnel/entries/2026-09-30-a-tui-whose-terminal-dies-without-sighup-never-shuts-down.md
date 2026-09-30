---
id: 2026-09-30-a-tui-whose-terminal-dies-without-sighup-never-shuts-down
date: 2026-09-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A TUI whose terminal dies while its launcher ignores SIGHUP either spins at
  100% CPU holding the vault lock or panics on its next render, and never
  runs the session shutdown.
---

## Bug
Found by the verifier of dogfooding phase 1, Increment 1d: with `trap '' HUP`
in the launching shell (what `nohup` and several launchers do), killing the pty
master left 5/5 TUIs running at 100% CPU with the vault's writer lock and an
acknowledged edit only in `.loro`. The red run of the new test showed a second
shape: the next render's `eprintln!` in r3bl
(`crossterm_paint_render_op_impl.rs:425`) panicked on the dead stderr
(`failed printing to stderr: Input/output error`) and the process died without
the shutdown.

## Root cause
The hangup reaches the shell, not the TUI, so no stop signal arrives.
crossterm's input reader treats the hung-up tty as always readable and never
reports an end, so r3bl's event loop never ends either.

## Missing piece
`frontends/tui/tests/stop_signals_quit_through_the_session_shutdown.rs` only
sent signals; no test closed the terminal under a launcher that ignores
SIGHUP.

## Remedy
The TUI polls stdin for `POLLHUP` every 200 ms
(`frontends/tui/src/terminal_hangup.rs`) and quits through `shutdown_session`
when it sees it. A render that wins the race cannot panic: while the TUI
runs, stderr goes to `tui.log` (`frontends/tui/src/stderr_to_log.rs`), so a
release build (`panic = "abort"`) also reaches the shutdown. Nothing catches
a panic, so dev and release take the same path. Pinned in
`frontends/tui/tests/stop_signals_quit_through_the_session_shutdown.rs` by
`a_dead_terminal_ends_the_tui_through_the_session_shutdown` (red when only the
hangup arm is removed, `lane-logs/dffu-r2-sab-final-hangup-and-first-then.log`)
and `a_draw_to_a_dead_terminal_ends_the_tui_through_the_session_shutdown`
(stdout and stderr on a dead terminal, stdin live; red when the redirect is
removed, `lane-logs/dffu-r3-sab.log`, S3).
