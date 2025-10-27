#!/usr/bin/env bash
# Watch holon-mcp sources and trigger Claude Code's MCP auto-reconnect on rebuild.
#
# Pairs with `.mcp.json` (type=stdio, command=cargo run --bin holon-mcp ...).
# Claude Code spawns the subprocess; this script:
#   1. Detects source changes
#   2. Rebuilds incrementally
#   3. Kills the running binary
#   4. Claude Code's auto-reconnect respawns via `cargo run` (binary is already fresh)
#
# Run this in a terminal alongside Claude Code. Ctrl-C stops watching.

set -euo pipefail

cd "$(dirname "$0")/.."

exec cargo watch \
    --watch crates \
    --watch frontends/mcp \
    --shell 'cargo build --quiet --bin holon-mcp && pkill -x holon-mcp || true'
