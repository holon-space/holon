---
id: 2026-11-24-quit-waits-on-a-pending-keychain-prompt
date: 2026-11-24
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  A frontend that quits while its device-key load waits on a platform keychain
  prompt logs "Shutdown complete" and then never exits.
---

## Bug
Found by a lane agent while it ran the holon-tui process tests. The spawned
`holon-tui` binaries logged "Shutdown complete" but stayed alive, reparented to
pid 1, still holding their vault and database. A stack sample of one of them
showed the cause.

## Root cause
- The device-key load runs on the blocking pool
  (`crates/holon-loro-wiring/src/device_key_net.rs:175`).
- With the login keychain granted and no default keychain reachable (a
  throwaway `HOME`), the store call blocks in `SecItemAdd` →
  `AuthorizationCopyRights`, waiting for the SecurityAgent dialog to be
  answered.
- `main` returns, and the `#[tokio::main]` runtime drop
  (`frontends/tui/src/main.rs:62`) waits without a limit for every blocking
  task. The process therefore outlives its own shutdown for as long as the
  dialog stays open.
- The same shape applies to every frontend that grants the keychain and
  runs on a tokio runtime with default shutdown: holon-gpui, holon-mcp,
  holon-tui.

## Missing piece
No test boots a binary against a keychain that blocks: every test boot holds
its secrets in memory, so the blocking platform path never runs under test,
and no check asserts that the process exits after "Shutdown complete".

## Remedy
Open.
- Bound the runtime's wait for blocking tasks at quit (for example
  `Runtime::shutdown_timeout`), and disclose an abandoned keychain call in the
  log instead of waiting on it.
- Pin it with a spawned-binary test whose keychain store blocks forever: the
  process must exit within a bound after "Shutdown complete".
