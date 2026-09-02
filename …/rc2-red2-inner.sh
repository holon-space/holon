#!/usr/bin/env bash
set -u
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/rc2-inert || exit 97
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
export RUSTC_WRAPPER=
exec /opt/homebrew/opt/rustup/bin/cargo nextest run -p holon-core -p holon \
  --features holon/test-helpers --no-fail-fast \
  -E 'test(integration_attribution) + test(ui_watcher)'
