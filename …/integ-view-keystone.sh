#!/usr/bin/env bash
set -euo pipefail
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/integ-view
export RUSTC_WRAPPER=
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
echo "=== keystone-smoke ==="
just keystone-smoke 2>&1 | tee lane-logs/integ-view-keystone.log || echo "KEYSTONE_EXIT=$?"
echo "=== hand-authored ==="
just hand-authored 2>&1 | tee lane-logs/integ-view-handauthored.log || echo "HANDAUTH_EXIT=$?"
