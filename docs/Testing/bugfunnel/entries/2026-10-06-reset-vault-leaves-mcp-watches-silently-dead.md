---
id: 2026-10-06-reset-vault-leaves-mcp-watches-silently-dead
date: 2026-10-06
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  The debug-only MCP tool reset_vault swaps the engine but keeps the MCP
  server's watch table, so a watch made before the reset keeps answering
  poll_watch with Ok from the retired engine and never delivers again.
---

## Bug

Found by the epoch-flip verifier (code reading only, not driven) while
checking whether the epoch-flip model fix hid a product defect. It is not
caused by that fix.

`reset_vault` (`frontends/mcp/src/tools.rs` ~:3836-3900, `#[cfg(debug_assertions)]`)
is the one production-code path where the engine swaps in-process while the MCP
server and its client sessions stay alive. A `watch_id` made before the reset
stays valid afterwards. `poll_watch` returns `Ok` from the RETIRED engine's
`pending_changes` stream, which idles against abandoned temp paths
(`RetiredSut`, `tools.rs:211-220`). The client gets a successful poll that can
never deliver another row and is never told to re-register.

## Root cause

`reset_vault` swaps the live backend cell and the `live_debug` handles. It
never touches `HolonMcpServer::watches` (`frontends/mcp/src/server.rs:417`).
The only accesses of `self.watches` in `tools.rs` are `watch_query`,
`poll_watch` and `stop_watch` (`:2757`, `:2786`, `:2812`). This is the
silent-degradation shape the fail-loud rule forbids.

## Missing piece

No transition in the keystone calls `reset_vault`, and the tool is debug-only,
so no sequence reaches a watch that survives an engine swap. The `Reboot`
transition restarts the whole boot, which drops the watches as production does,
so it does not model this path.

## Remedy

Open, deliberately not fixed in this lane. Candidate fixes: clear `watches` in
`reset_vault`, or make `poll_watch` return an error when the watch belongs to a
retired engine. Either needs a red-first case that registers a watch, calls
`reset_vault`, then polls.
