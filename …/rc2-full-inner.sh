#!/usr/bin/env bash
set -u
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/rc2-inert || exit 97
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
export RUSTC_WRAPPER=
/opt/homebrew/opt/rustup/bin/cargo nextest run -p holon -p holon-mcp-client \
  --features holon/test-helpers --no-fail-fast
echo "full-exit=$?"
