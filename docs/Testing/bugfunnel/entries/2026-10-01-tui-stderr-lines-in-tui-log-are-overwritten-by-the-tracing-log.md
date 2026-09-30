---
id: 2026-10-01-tui-stderr-lines-in-tui-log-are-overwritten-by-the-tracing-log
date: 2026-10-01
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  With HOLON_LOG unset, the tracing log overwrote every stderr line that
  holon-tui sent to tui.log, including the std panic text.
---

## Bug
Found by the round-2 verifier of the dogfood follow-ups lane
(`lane-logs/dffu-r2-verify.md`, DEFECT 1): under `HOLON_DEBUG_CHORD=1`, Alt+i
wrote `[chord]` to tui.log, and 3 s later the line was gone. The std panic
text was also missing from tui.log.

## Root cause
Two descriptors wrote the same file with different offsets. Tracing opened
tui.log with `File::create` (`crates/holon-frontend/src/logging.rs`, the
`LogDest::File` arm), a non-append handle with its own offset.
`StderrToLog` (`frontends/tui/src/stderr_to_log.rs`) opened it `O_APPEND`.
Each stderr line sat past the tracing offset, and the next tracing write
overwrote it.

## Missing piece
The TUI process harness (`frontends/tui/tests/tui_process/mod.rs`) always set
`HOLON_LOG` to a separate file, so no test ran the production log layout.

## Remedy
A `file://` log destination opens in append mode (truncated once at open,
created `0600`), so both writers append. The harness no longer sets
`HOLON_LOG`: every TUI process test runs the production layout.
`a_stderr_write_while_the_tui_draws_lands_in_the_log` checks the stderr line
after the shutdown's log writes; it is red when only the append mode is
removed (`lane-logs/dffu-r3-sab.log`, S1).
