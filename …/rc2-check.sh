#!/usr/bin/env bash
set -u
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/rc2-inert || exit 97
test -f crates/holon-core/src/integration_attribution.rs || exit 98
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
export RUSTC_WRAPPER=
echo "tree: $(pwd)"
/opt/homebrew/opt/rustup/bin/cargo --version
/opt/homebrew/opt/parallel/bin/parallel --semaphore --id holon-build -j4 --fg -- \
  /opt/homebrew/opt/rustup/bin/cargo check --workspace --all-targets \
  --features holon-integration-tests/pbt,holon-gpui/pbt
echo "check-exit=$?"
