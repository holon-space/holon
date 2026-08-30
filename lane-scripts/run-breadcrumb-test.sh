#!/bin/bash
set -uo pipefail
export PATH="$HOME/.rustup/toolchains/nightly-2026-07-17-aarch64-apple-darwin/bin:$HOME/.cargo/bin:$PATH"
export RUSTC_WRAPPER=
LANE=/Users/martin/Workspaces/pkm/holon/.claude/worktrees/breadcrumb-root
cd "$LANE" || exit 97
pwd
LOG="$1"
shift
LOCK=/tmp/holon-breadcrumb-build.lock
while ! mkdir "$LOCK" 2>/dev/null; do sleep 5; done
trap 'rmdir "$LOCK"' EXIT
cargo nextest run -p holon-gpui "$@" --features holon-gpui/pbt --no-fail-fast \
  > "$LOG" 2>&1
echo "exit=$?"
