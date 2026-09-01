#!/usr/bin/env bash
set -u
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/rc2-inert || exit 97
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
export RUSTC_WRAPPER=
exec /opt/homebrew/opt/rustup/bin/cargo nextest run \
  -p holon-core -p holon-mcp-client -p holon-turso -p holon -p holon-frontend -p holon-app \
  --features holon/test-helpers --no-fail-fast \
  -E 'test(integration_attribution) + test(command_resolution) + binary(inert_integration_disclosure) + binary(missing_deps_error_stays_typed) + test(ui_watcher) + test(dead_sidecar) + test(missing_from_path)'
