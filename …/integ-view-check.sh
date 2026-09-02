#!/usr/bin/env bash
set -euo pipefail
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/integ-view
export RUSTC_WRAPPER=
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
rustup show active-toolchain
cargo check -p holon-api -p holon-mcp-client -p holon-turso -p holon-app --all-targets 2>&1 | tee lane-logs/integ-view-check.log
