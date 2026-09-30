---
id: 2026-09-30-a-refused-write-back-summary-is-lost-with-the-terminal
date: 2026-09-30
gap: ORACLE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  When the TUI's terminal closes, the shutdown's named summary of the org
  files it could not write goes to the dead terminal and is recorded nowhere.
---

## Bug
Found by the verifier of dogfooding phase 1, Increment 1d, with a probe: a
TUI on a pty, an acknowledged edit into a read-only directory, then the pty
master killed. 4/4 runs shut down and exited, the edit was in `.loro` and not
in the org file, and the summary `refused the org write-back … not in these
files` was in no file. Only the per-block ERROR line reached the log.
`holon-mcp` in stdio mode had the same shape: the summary went to stderr only.

## Root cause
`shutdown_session` returns the refusal as its `Err` without logging it
(`crates/holon-app/src/session.rs`, `wait_for_writeback`). `holon-tui` and
`holon-mcp` returned that `Err` from `main`, which prints it to stderr, and
logged nothing. GPUI's `quit` logs it first.

## Missing piece
`crates/holon-app/tests/shutdown_writes_back_every_edit.rs::a_refused_write_back_fails_the_shutdown_by_name`
checks the error value, and no test checked that a binary records it where a
user without a terminal can read it.

## Remedy
Both binaries log the error they exit with (`frontends/tui/src/main.rs`,
`frontends/mcp/src/main.rs`). Pinned by
`frontends/tui/tests/stop_signals_quit_through_the_session_shutdown.rs::a_refused_write_back_is_in_the_log_after_the_terminal_hung_up`
and `frontends/mcp/tests/sigterm_shuts_the_session_down.rs::a_refused_write_back_is_in_the_log`,
each red before the fix.
