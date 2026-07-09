#!/usr/bin/env bash
# Run the composed keystone PBT (`general_e2e_composed_pbt_live_mcp`) against a
# LIVE Holon app running on the iOS simulator, driven over its embedded MCP
# server. This is the out-of-process twin of the headless keystone — same
# transitions + invariant catalog, but every step goes through the real app.
#
# PREREQUISITES
#   - The Holon iOS app is already built + installed on the target simulator.
#     This script only (re)launches it; it never rebuilds/reinstalls.
#   - `xcrun`, `idb` (optional, for taps) on PATH.
#
# WHY the relaunch: the keystone does a per-case `reset_vault`, which the app
# refuses unless it was launched with HOLON_MCP_ALLOW_RESET=1. The app also must
# serve MCP on $MCP_SERVER_PORT. `simctl launch` forwards SIMCTL_CHILD_* env
# into the app process, so we relaunch with both set.
#
# STATUS (2026-07-09): the gate is NOT yet green. The keystone connects, resets,
# and drives transitions for ~50s, then fails on the `SplitBlock` transition:
# the live driver geometry-clicks the split target block by its BoundsRegistry
# bounds, but a block that lives under a page other than the focused main-panel
# root is never rendered, so `click_entity` reports "no bounds recorded" and the
# 10s budget in `focus_editor` (live_mcp.rs) expires. See the FINAL REPORT /
# BugFunnel. To reach green the live driver must navigate the target block's
# page into `main` (or focus it) before geometry-driving it. Run this script to
# reproduce the blocker and to re-check once that harness gap is closed.

set -euo pipefail

SIM_UDID="${SIM_UDID:-8F94025E-F5A4-48FE-8E49-9E7FBCB19DBB}"
BUNDLE_ID="${BUNDLE_ID:-space.holon.gpui}"
MCP_PORT="${MCP_SERVER_PORT:-8521}"
CASES="${PROPTEST_CASES:-3}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG="${LOG:-/tmp/ios_live_mcp_e2e.log}"

echo "==> Relaunching $BUNDLE_ID on $SIM_UDID with reset enabled + MCP on :$MCP_PORT"
xcrun simctl terminate "$SIM_UDID" "$BUNDLE_ID" >/dev/null 2>&1 || true
sleep 1
SIMCTL_CHILD_MCP_SERVER_PORT="$MCP_PORT" \
SIMCTL_CHILD_HOLON_MCP_ALLOW_RESET=1 \
  xcrun simctl launch "$SIM_UDID" "$BUNDLE_ID"

echo "==> Waiting for MCP to answer on 127.0.0.1:$MCP_PORT ..."
for i in $(seq 1 15); do
  sleep 2
  if curl -s -o /dev/null -m 2 \
       -H 'Content-Type: application/json' \
       -H 'Accept: application/json, text/event-stream' \
       -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"gate","version":"0"}}}' \
       "http://127.0.0.1:$MCP_PORT/mcp"; then
    echo "    MCP up after ~$((i * 2))s"
    break
  fi
done

echo "==> Running general_e2e_composed_pbt_live_mcp (PROPTEST_CASES=$CASES) — log: $LOG"
cd "$REPO_ROOT"
# NB: NOT `--ignored`. The live_mcp test is a plain #[test] that self-skips
# unless HOLON_PBT_LIVE_MCP is set; `--ignored` would filter it out.
HOLON_PBT_LIVE_MCP=1 \
MCP_SERVER_PORT="$MCP_PORT" \
PROPTEST_CASES="$CASES" \
  cargo test -p holon-integration-tests \
    --test general_e2e_composed_pbt general_e2e_composed_pbt_live_mcp \
    -- --nocapture --test-threads=1 2>&1 | tee "$LOG"
