#!/bin/bash
set -uo pipefail
export PATH="$HOME/.rustup/toolchains/nightly-2026-07-17-aarch64-apple-darwin/bin:$HOME/.cargo/bin:$PATH"
export RUSTC_WRAPPER=
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/breadcrumb-root || exit 97
pwd
bash scripts/keystone-known-reds-fixture.sh > lane-logs/gate-known-reds-fixture.log 2>&1
echo "fixture=$?"
