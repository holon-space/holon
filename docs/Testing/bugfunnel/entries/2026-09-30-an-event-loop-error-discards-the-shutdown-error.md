---
id: 2026-09-30-an-event-loop-error-discards-the-shutdown-error
date: 2026-09-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  When the TUI event loop or the MCP server fails, the session shutdown's own
  error is dropped unlogged.
---

## Bug
Found by code read (verifier of dogfooding phase 1, Increment 1d, finding
F3). No probe reached it.

## Root cause
`frontends/tui/src/main.rs` ran `ran?` before returning the shutdown result,
and `frontends/mcp/src/main.rs` ran `served?` the same way, so a failing event
loop or server replaced the shutdown's error, which names any org file left
unwritten.

## Missing piece
No test makes the event loop or the server fail together with the shutdown.
No binary test reaches that branch: no probe made both fail at once.

## Remedy
Both binaries return `holon_app::session::first_then(ran, shutdown)`
(`crates/holon-app/src/session.rs`), which keeps both errors
(`"{ran}; then {shutdown}"`), and log the error they exit with. Pinned by the
unit test `session::first_then_tests::the_exit_status_keeps_every_error`, red
when `first_then` is the old `first?; then` shape
(`lane-logs/dffu-r2-sab-final-hangup-and-first-then.log`). The logging is
pinned by the entry
`2026-09-30-a-refused-write-back-summary-is-lost-with-the-terminal`.
