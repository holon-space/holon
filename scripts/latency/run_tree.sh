#!/usr/bin/env bash
# One tree's full set_field latency measurement: fresh sandbox, fresh app
# (launching makes the window frontmost, which is the condition the SLO is
# about), one probe block, both arrival arms. Run one tree at a time so the two
# trees of an A/B see the same machine.
#
#   run_tree.sh <tree-root> <mcp-port> <label> <out-dir>
#
# <tree-root> must already contain target/debug/holon-gpui (see build-target.sh).
set -euo pipefail
TREE="$1"; PORT="$2"; LABEL="$3"; OUT="$4"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SB="/tmp/holon-latency-${PORT}"

mkdir -p "$OUT"
rm -rf "$SB"; mkdir -p "$SB/config" "$SB/vault"
HOLON_CONFIG_DIR="$SB/config" HOLON_VAULT_ROOT="$SB/vault" MCP_SERVER_PORT="$PORT" \
  HOLON_MCP_ALLOW_RESET=1 RUST_LOG=holon_latency=debug,info \
  nohup "$TREE/target/debug/holon-gpui" > "$SB/app.log" 2>&1 &
APP=$!
echo "$APP" > "$SB/app.pid"
for _ in $(seq 1 90); do
  curl -sf "http://127.0.0.1:$PORT/health" > /dev/null && break
  sleep 1
done
curl -sf "http://127.0.0.1:$PORT/health" > /dev/null || { tail -20 "$SB/app.log"; exit 1; }

M="python3 $HERE/mcp.py $PORT"
DAY=$($M execute_raw_sql '{"sql":"SELECT id FROM block_raw WHERE content LIKE '"'"'2026-%'"'"' AND parent_id IS NOT NULL LIMIT 1"}' \
      | grep -o 'block:[0-9a-f-]*' | head -1)
echo "day=$DAY"
$M execute_operation "{\"entity_name\":\"block\",\"operation\":\"create\",\"params\":{\"parent_id\":\"$DAY\",\"content\":\"latency probe target\"}}"
PROBE=$($M execute_raw_sql '{"sql":"SELECT id FROM block_raw WHERE content LIKE '"'"'%latency probe%'"'"'"}' \
        | grep -o 'block:[0-9a-f-]*' | head -1)
echo "probe=$PROBE"
# The probe block needs one paint before its bounds are clickable.
$M click "{\"entity_id\":\"$DAY\"}" || true
sleep 2
$M screenshot '{}' > /dev/null
sleep 2

python3 "$HERE/measure_arms.py" --port "$PORT" --log "$SB/app.log" \
  --block "$PROBE" --n 32 --tree "$LABEL" --out "$OUT/arms-${LABEL}.json"

kill "$APP" 2> /dev/null || true
echo "RUN_SUMMARY_OK $LABEL  app.log=$SB/app.log  json=$OUT/arms-${LABEL}.json"
