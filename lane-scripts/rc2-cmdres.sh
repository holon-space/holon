#!/usr/bin/env bash
set -u
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/rc2-inert || exit 97
test -f crates/holon-mcp-client/src/command_resolution.rs || exit 98
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
export RUSTC_WRAPPER=
echo "toolchain: $(rustup show active-toolchain)"
echo "tree: $(pwd)"
/opt/homebrew/opt/parallel/bin/parallel --semaphore --id holon-build -j4 --fg -- \
  /opt/homebrew/opt/rustup/bin/cargo nextest run -p holon-mcp-client command_resolution --no-fail-fast
echo "exit=$?"
