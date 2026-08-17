---
id: 2026-08-17-enable-script-reported-success-for-a-provider-that-does-not-exist
date: 2026-08-17
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  scripts/holon-integration-enable.sh gmial exited 0 printing "Enabled 'gmial'",
  writing a state file nothing ever reads.
---

## Bug

Found by the fresh-context verifier on the D4.b enablement-cutover lane (task
#50), before landing. `scripts/holon-integration-enable.sh gmial` — one typo
away from `gmail` — created `gmial.state.toml`, printed `Enabled 'gmial'` and
exited 0. The store only ever looks up the providers this build ships, so the
file was read by nothing and disclosed by nothing. The user walks away believing
an integration is on.

## Root cause

Presence is a compile-time fact (`crates/holon-mcp-client/src/bundled_sidecars.rs`),
but the script validated nothing against it — it wrote whatever name it was
given. The loader had the same blind spot from the other side:
`IntegrationConfigStore::load` iterates `BUNDLED_SIDECARS`, so a `*.state.toml`
for an unknown name is never even opened.

## Missing piece

The enable script had exactly one test, and it exercised the happy path with a
real provider. Nothing ran it with a wrong input, and no loader test placed a
state file for a name the build does not ship.

## Remedy

Both sides, since either alone leaves a silent hole:

- The script parses the bundled providers out of `bundled_sidecars.rs` and
  refuses an unknown name, naming what it could have meant.
- `load_integration_configs` scans for orphan `*.state.toml` files and reports
  each as `IgnoredSidecar { reason: NotBundled }`, so a hand-written one is
  disclosed at boot like any other file that enables nothing.

Red-first, `/tmp/lane-enablement-red3.log`:
`the_enable_script_refuses_a_provider_this_build_does_not_ship` (exited 0) and
`an_orphan_state_file_is_disclosed` (no entry). Both green after.
