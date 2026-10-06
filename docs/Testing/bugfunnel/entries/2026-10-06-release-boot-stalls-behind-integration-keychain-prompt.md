---
id: 2026-10-06-release-boot-stalls-behind-integration-keychain-prompt
date: 2026-10-06
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  One integration whose connect never completes (on the live release boot, the
  last one blocked on a keychain prompt) keeps FrontendSession from resolving,
  so the embedded MCP server never listens and the post-scan sync gate never opens.
---

## Bug

Martin's first release build of holon-gpui (d09d95c5 + c77f42c9) booted on his
vault. The org initial scan finished in 37 s. The embedded MCP server never
bound port 8520. All integrations logged `sync still deferred` every 60 s until
the 600 s watchdog. Found by dogfooding; triaged by lane `release-boot`.

## Root cause

The boot log never has `[FrontendSession] factory: BackendEngine resolved`,
`[OperationModule] Found N operation providers` or
`[McpIntegrationsModule] Registry created`. The last line in the
`resolve_engine` span is `Provider 'todoist' connected` (21:04:55.98).

- `crates/holon-app/src/wiring.rs:502` — the `FrontendSession` factory awaits
  `BackendEngine`. The engine factory resolves the `OperationDispatcher`, which
  collects every `OperationProvider`. Each MCP provider factory awaits the
  `McpIntegrationRegistry` (`crates/holon-app/src/mcp_integrations.rs:891`).
- `crates/holon-app/src/mcp_integrations.rs:672` — the registry connects every
  integration serially, with no bound on a connect.
- `crates/holon-mcp-client/src/roster.rs` — installed sidecars come after the
  bundled ones, so the installed `github` connection is the last one. Its
  `${GITHUB_TOKEN}` resolves env → keychain → preference
  (`crates/holon-frontend/src/integration_vars.rs:50-55`). The keychain read is
  a synchronous `get_generic_password` (`crates/holon-secrets/src/mac.rs:36`).
- A `SecurityAgent` process with an on-screen window started at 21:04:56 UTC,
  0.02 s after the todoist line. A new release binary has a new code identity,
  so the keychain item's ACL asks again. Martin's process has no socket to
  GitHub. The link "prompt ← github token read" is inferred from timing and
  order; the dialog text was not read.
- Consequence: `GpuiModule::on_start` (`frontends/gpui/src/di.rs:119`) waits on
  the session and never calls `mcp.start` (`:141`). `post_ready` (`wiring.rs:852`, the
  only opener of the sync gate at `:885`) is spawned only after the engine
  resolves, so the gate falls back to the 600 s watchdog.
- The sync gate itself is a `watch` channel: no lost wake-up (hypothesis H1
  refuted).

Reproduction without the keychain: an installed MCP HTTP sidecar whose peer
accepts and never answers. Release binary on an empty vault and on a copy of
the vault, both without integrations: MCP up in under 3 s.

## Missing piece

No test boots the production wiring with an integration whose connect, or
whose credential lookup, does not complete. The fake MCP module and every
integration test connect at once, so "integration setup is on the session's
critical path" was never observable.

## Remedy

Red test: `crates/holon-integration-tests/tests/frontend_suite/integration_connect_stall_blocks_boot.rs`.

OPEN — design question. A timeout around the async connect does not cover the
keychain case: the read blocks a runtime worker thread. Candidate shapes: take
the registry's connect loop off the `BackendEngine` resolve (providers register
when they connect), or read secrets non-interactively at boot and disclose the
missing approval.
