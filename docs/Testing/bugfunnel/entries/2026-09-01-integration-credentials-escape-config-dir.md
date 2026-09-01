---
id: 2026-09-01-integration-credentials-escape-config-dir
date: 2026-09-01
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Integration sidecars resolve credentials from $HOME/.config/holon regardless of
  HOLON_CONFIG_DIR, so a sandboxed instance silently authenticated against the
  real Google account and synced 21 real calendar events into a throwaway DB.
---

## Bug

Found by `dogfood-explorer` pass #2 over v0.0.23 (`d49ef0316a77`), driving the
GPUI app through its embedded MCP server on port 8720.

The instance was launched under the dogfood safety protocol
(`.claude/skills/dogfood-explorer/SKILL.md` §0): `HOLON_CONFIG_DIR` and
`HOLON_VAULT_ROOT` both pointed inside a freshly-created throwaway directory,
and the boot line confirmed every path was inside the sandbox. The five bundled
integrations were enabled with `scripts/holon-integration-enable.sh`, which
records `status = "unconfigured"` when given no credential paths.

Despite `gcal` being enabled as **unconfigured** in a sandbox config dir, the
sidebar reported it `Connected` and `SELECT count(*) FROM gcal_event` returned
**21 rows of the real user's actual calendar** — genuine Google event ids,
real meeting titles, real conferencing links. None of this data was seeded; the
sandbox database was created at boot.

The isolation that the whole dogfood protocol depends on does not hold: pointing
Holon at a throwaway config dir does not stop it from reaching real user
accounts over the network.

## Root cause

`assets/integrations/gcal.yaml:127-133` hardcodes the credential locations as
tilde paths:

```yaml
client_id_file: ~/.config/holon/gcal-client-id
client_secret_file: ~/.config/holon/gcal-client-secret
refresh_token_file: ~/.config/holon/gcal-refresh-token
```

The comment directly above states "A leading `~/` expands to `$HOME`". These
paths are resolved against `$HOME`, **not** against the app's resolved config
dir (`crates/holon-frontend/src/config.rs::resolve_config_dir`, which honours
`HOLON_CONFIG_DIR`). So the OAuth refresh token in the developer's real
`~/.config/holon/` is picked up by any instance on the machine, whatever
`HOLON_CONFIG_DIR` says, and the REST transport performs a real authenticated
sync.

The same mechanism is visible in the failure direction for `gmail`: its startup
error names an absolute path under the real home directory rather than anything
inside the sandbox.

Two consequences beyond the isolation break:

- `status = "unconfigured"` in `{config_dir}/integrations/<p>.state.toml` is not
  a gate. It does not prevent a network sync; the provider simply resolves
  ambient credentials and reports `Connected`.
- The status column is derived from live connectivity, so the sidebar shows a
  provider as `Connected` that the user never configured *in this profile*.

Evidence: `/tmp/dogfood2-0901/logs/app3.log`; `integration_state` and
`gcal_event` queried live over MCP. Real calendar content is deliberately not
reproduced here.

## Missing piece

No test asserts that a Holon instance confined to a config dir stays confined.
The keystone runs headless with no MCP-client integrations wired at all, so the
credential-resolution path never executes in the test environment — a textbook
ENVIRONMENT escape. There is also no boot-time guard that refuses, or even
warns, when a sidecar resolves a credential file outside the active config dir.

## Remedy

Open. Proposed, in order:

1. Resolve sidecar `*_file` credential paths against the **active config dir**,
   not `$HOME`; treat a bare `~/` as a config-dir-relative reference, or refuse
   it loudly.
2. Add a boot guard that fails loud when a credential path escapes the active
   config dir, so the sandbox breach is impossible to miss.
3. Honour `status = "unconfigured"` as an actual gate on syncing.
4. Add the fourth row to the `dogfood-explorer` §0 safety table: config-dir
   override does **not** isolate integration credentials. Until (1) lands, a
   dogfood launch must also neutralise `$HOME` (or disable OAuth providers) to
   avoid touching real accounts.

Until fixed, treat every dogfood or dev launch on a machine with configured
integrations as touching production accounts.
