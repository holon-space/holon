#!/usr/bin/env bash
set -euo pipefail
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/integ-view
export RUSTC_WRAPPER=
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
echo "=== fmt ==="
cargo fmt --all -- --check
echo "=== nextest holon-mcp-client + holon-app ==="
cargo nextest run -p holon-mcp-client -p holon-app --no-fail-fast 2>&1 | tee lane-logs/integ-view-rc1b-confirm.log
