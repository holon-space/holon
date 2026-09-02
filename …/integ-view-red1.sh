#!/usr/bin/env bash
set -euo pipefail
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/integ-view
export RUSTC_WRAPPER=
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
cargo nextest run -p holon-app --test integration_state_projection 2>&1 | tee lane-logs/integ-view-red1-tablecolumns.log
