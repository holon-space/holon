---
id: 2026-08-31-bundled-sidecar-hardcodes-developer-local-binary-path
date: 2026-08-31
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  The bundled `claude-history` sidecar names an absolute path into one
  developer's debug `target/` directory, so the integration cannot connect on
  any machine where that build does not exist.
---

## Bug

Found by Martin opening the `ClaudeCode` page in the live GPUI app
(2026-08-31 cold boot, `/tmp/holon-cold.log`, 58 MB). The page rendered no
data and the log carried five `render_entity(...) failed` errors.

Boot log, 6 hours before the page was opened:

```
02:20:16.291279Z WARN  [McpIntegrationsModule] Failed to connect provider
  'claude-history': No such file or directory (os error 2)
02:20:18.468586Z WARN  [McpIntegrationsModule] Integration 'claude-history'
  unavailable (not configured or failed to connect) — registering inert
  provider; cause was disclosed on the degraded bus at boot
```

## Root cause

`assets/integrations/claude-history.yaml:8` sets

```yaml
command: /Users/martin/Workspaces/ai/claude-code-history-mcp/target/debug/claude-code-history-mcp
```

The repository at that path exists; the `target/debug` binary does not. The
spawn fails with ENOENT at
`crates/holon-app/src/mcp_integrations.rs:573-583`, the provider is recorded
`Unavailable`, and an `EmptyOperationProvider` is registered instead
(`mcp_integrations.rs:625-635`).

The path is baked into a repository asset, so this is not a local
misconfiguration: the bundled sidecar is unusable for every user, every CI
run, and every fresh checkout. It only ever worked on the one machine that
had built that binary into that directory.

Note the drift guard added by
`2026-08-08-permanently-unavailable-because-installed-sidecar-has` DID work
here — the installed copy declared no `schema_version`, was rejected loudly,
and the bundled copy was used instead
(`crates/holon-mcp-client/src/bundled_sidecars.rs`). The bundled copy then
failed for this separate reason.

## Missing piece

No test or gate asserts that a bundled sidecar's `transport.child_process.command`
is resolvable from the repository — neither as a workspace-relative path, a
`${VAR}` the environment supplies, nor a `PATH` lookup. An absolute path into a
`target/` directory outside the workspace is accepted verbatim.

## Remedy

Open. Fix direction: the bundled sidecar must not name a machine-specific
path. Either resolve the provider binary through `PATH`/a `${VAR}` the app
documents, or ship the sidecar with a placeholder that the loader refuses at
parse time with a message naming the unresolved binary. Add a cheap unit test
over `assets/integrations/*.yaml` asserting every `command` is either a bare
program name, a workspace-relative path, or a `${VAR}` — never an absolute
path under a `target/` directory. Pairs with
`2026-08-31-inert-integration-registers-no-schema-provider`, which is what
turned this boot-time failure into five opaque page errors.
