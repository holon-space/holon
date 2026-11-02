---
id: 2026-11-02-tui-test-orphan-holds-fixed-mcp-port
date: 2026-11-02
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  A tui_process test that panicked left holon-tui serving MCP on its fixed
  port, and every later run of a test on that port talked to the orphan.
---

## Bug
A verifier of another lane found an orphan `holon-tui` listening on
127.0.0.1:18765 after a lane's nextest run. From then on,
`a_refused_write_back_is_in_the_log_after_the_terminal_hung_up`, in any lane
workspace, connected to the orphan, got an ack for an edit into the orphan's
vault, and failed at "a shutdown that left an edit out of its org file must
exit non-zero".

## Root cause
- `Tui` in `frontends/tui/tests/tui_process/mod.rs` held the `script` wrapper
  as a `Child` and had no `Drop`. A panic dropped the `Child` without a kill;
  `script` went to pid 1 and `holon-tui` kept running.
- Each test passed a fixed `MCP_SERVER_PORT` (18761..18771, two of them used
  twice), so a later run's client reached whatever server held that port.

## Missing piece
The test harness had no cleanup on unwind and no isolation from servers of
other runs: the failing path (a test process that dies mid-test while another
run still holds the port) exists only on a shared developer or CI machine.

## Remedy
- `impl Drop for Tui` kills the process group of the TUI (`killpg` on the
  session leader that `script` starts) and then `script`.
- `Launch::new()` takes no port; each instance gets a port the OS has free at
  spawn time, and tests reach it with `Tui::mcp()`.
- Pinned by `tui_process::isolation::a_panicking_test_leaves_no_tui_and_no_listener_behind`
  and `tui_process::isolation::the_mcp_client_never_reaches_a_listener_that_another_run_holds`.
