#!/usr/bin/env bash
set -euo pipefail
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/integ-view
export RUSTC_WRAPPER=
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
echo "=== fmt ==="
cargo fmt --all -- --check 2>&1 | tee lane-logs/integ-view-fmt.log
echo "=== workspace check ==="
cargo check --workspace --all-targets 2>&1 | tee lane-logs/integ-view-wscheck.log
echo "=== lane tests ==="
cargo nextest run -p holon-app -p holon-mcp-client -p holon-turso -p holon-api -p holon-pattern --no-fail-fast 2>&1 | tee lane-logs/integ-view-tests.log
