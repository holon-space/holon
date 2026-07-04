#!/usr/bin/env bash
# Chunked runner for the live-MCP keystone PBT against the iOS sim app.
#
# reset_vault retires engines in-process (cap 20/launch), so a long generated
# run must be chunked: cold-relaunch the app (ios_reset_sut.sh) between chunks
# of PROPTEST_CASES <= CHUNK. On a failing case proptest persists the regression
# seed; replay + shrink it IN-PROC (HOLON_PBT_FORCE_FULL=1, no env gate), then
# re-verify the shrunk sequence live once.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

UDID="${IOS_SIM_UDID:-}"
PORT="${MCP_SERVER_PORT:-8521}"
CHUNK="${CHUNK:-8}"
CHUNKS="${CHUNKS:-2}"
TEST_FILTER="${TEST_FILTER:-live_mcp}"

if [ "$CHUNK" -gt 16 ]; then
  echo "CHUNK=$CHUNK too close to the 20-reset cap; use <=16" >&2
  exit 1
fi

for i in $(seq 1 "$CHUNKS"); do
  echo "=== chunk $i/$CHUNKS: cold relaunch + $CHUNK cases ==="
  HOLON_MCP_ALLOW_RESET=1 IOS_SIM_UDID="$UDID" \
    "$SCRIPT_DIR/ios_reset_sut.sh" --port "$PORT"
  (
    cd "$REPO_ROOT"
    HOLON_PBT_LIVE_MCP=1 MCP_SERVER_PORT="$PORT" PROPTEST_CASES="$CHUNK" \
      cargo test -p holon-integration-tests --features pbt \
      --test general_e2e_composed_pbt -- "$TEST_FILTER" --nocapture
  )
done
echo "=== all $CHUNKS chunks green ==="
