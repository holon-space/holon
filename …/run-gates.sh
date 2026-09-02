#!/bin/bash
set -uo pipefail
export PATH="$HOME/.rustup/toolchains/nightly-2026-07-17-aarch64-apple-darwin/bin:$HOME/.cargo/bin:$PATH"
export RUSTC_WRAPPER=
LANE=/Users/martin/Workspaces/pkm/holon/.claude/worktrees/breadcrumb-root
cd "$LANE" || exit 97
pwd
LOCK=/tmp/holon-breadcrumb-build.lock
while ! mkdir "$LOCK" 2>/dev/null; do sleep 5; done
trap 'rmdir "$LOCK"' EXIT

cargo check -p holon-gpui --features holon-gpui/pbt > lane-logs/gate-check-gpui.log 2>&1
echo "check-gpui=$?"
cargo check -p holon > lane-logs/gate-check-holon.log 2>&1
echo "check-holon=$?"
just keystone-smoke > lane-logs/gate-keystone-smoke.log 2>&1
echo "keystone-smoke=$?"
uv run --with pyyaml python3 scripts/bugfunnel.py check > lane-logs/gate-bugfunnel.log 2>&1
echo "bugfunnel=$?"
