#!/usr/bin/env bash
# Per-case iOS SUT reset for the McpUserDriver rung (plan Phase 1, Option B').
#
# Deterministically resets the running Holon iOS-simulator app to a known seed
# so a replay/generated test case starts from a fixed, oracle-aligned state:
#
#   1. terminate the app
#   2. wipe the app container's org root (Documents/holon-pkm) + SQL db (Library/holon.db*)
#   3. copy a fixed seed dir into the org root (fixed `:ID:` drawers → ids match the
#      oracle by construction; no default seeding by `ios_data_paths` because the
#      org root is non-empty)
#   4. relaunch with the MCP port pinned
#   5. wait for MCP, then probe `block_raw`'s id-set and print it (the Phase-1 exit check)
#
# Why a host script and not in-test Rust: the reset acts on the app process from
# OUTSIDE (simctl terminate/wipe/launch); the McpUserDriver connects to the
# already-running app over MCP. Call this between cases (or shell out to it).
#
# Usage:
#   ios_reset_sut.sh [--udid UDID] [--bundle BUNDLE] [--port PORT] [--seed DIR]
# Defaults: UDID=$IOS_SIM_UDID, bundle=space.holon.gpui, port=8521,
#           seed=<this script dir>/seed_wide
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UDID="${IOS_SIM_UDID:-}"
BUNDLE="space.holon.gpui"
PORT="8521"
SEED_DIR="$SCRIPT_DIR/seed_wide"

while [ $# -gt 0 ]; do
  case "$1" in
    --udid) UDID="$2"; shift 2 ;;
    --bundle) BUNDLE="$2"; shift 2 ;;
    --port) PORT="$2"; shift 2 ;;
    --seed) SEED_DIR="$2"; shift 2 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

if [ -z "$UDID" ]; then
  UDID="$(xcrun simctl list devices booted | sed -nE 's/.*\(([0-9A-Fa-f-]{36})\) \(Booted\).*/\1/p' | head -1)"
fi
[ -n "$UDID" ] || { echo "ERROR: no booted simulator; pass --udid or set IOS_SIM_UDID" >&2; exit 1; }
[ -d "$SEED_DIR" ] || { echo "ERROR: seed dir not found: $SEED_DIR" >&2; exit 1; }

echo "[ios-reset] udid=$UDID bundle=$BUNDLE port=$PORT seed=$SEED_DIR"

# 1. terminate (ignore "not running")
xcrun simctl terminate "$UDID" "$BUNDLE" 2>/dev/null || true
sleep 1

# 2. wipe container org root + db
DATA="$(xcrun simctl get_app_container "$UDID" "$BUNDLE" data)"
ORG="$DATA/Documents/holon-pkm"
DB="$DATA/Library/holon.db"
rm -rf "$ORG"
mkdir -p "$ORG"
rm -f "$DB" "$DB-wal" "$DB-shm"
echo "[ios-reset] wiped $ORG and $DB*"

# 3. drop the fixed seed
cp "$SEED_DIR"/*.org "$ORG"/
echo "[ios-reset] seeded: $(cd "$ORG" && ls *.org | tr '\n' ' ')"

# 4. relaunch with MCP pinned
SIMCTL_CHILD_MCP_SERVER_PORT="$PORT" xcrun simctl launch "$UDID" "$BUNDLE" >/dev/null
echo "[ios-reset] launched"

# 5. wait for MCP
for _ in $(seq 1 40); do
  code="$(curl -s -m 2 -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/mcp" 2>/dev/null || true)"
  if [ "$code" = "406" ] || [ "$code" = "400" ] || [ "$code" = "200" ]; then
    echo "[ios-reset] MCP up (http $code)"; break
  fi
  sleep 1
done

# 6. id-set probe (the Phase-1 exit check)
python3 - "$PORT" <<'PY'
import json, sys, urllib.request
port = sys.argv[1]
base = f"http://127.0.0.1:{port}/mcp"; sid = {"id": None}
def post(body):
    d = json.dumps(body).encode()
    r = urllib.request.Request(base, data=d, method="POST")
    r.add_header("Content-Type", "application/json")
    r.add_header("Accept", "application/json, text/event-stream")
    if sid["id"]: r.add_header("Mcp-Session-Id", sid["id"])
    resp = urllib.request.urlopen(r, timeout=30)
    if resp.headers.get("Mcp-Session-Id"): sid["id"] = resp.headers.get("Mcp-Session-Id")
    out = None
    for ln in resp.read().decode().splitlines():
        ln = ln.strip()
        if ln.startswith("data:"): out = json.loads(ln[5:].strip())
        elif ln.startswith("{"): out = json.loads(ln)
    return out
post({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"ios-reset","version":"1"}}})
n = json.dumps({"jsonrpc":"2.0","method":"notifications/initialized"}).encode()
rn = urllib.request.Request(base, data=n, method="POST")
rn.add_header("Content-Type","application/json"); rn.add_header("Accept","application/json, text/event-stream"); rn.add_header("Mcp-Session-Id", sid["id"])
urllib.request.urlopen(rn, timeout=10).read()
res = post({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"execute_raw_sql","arguments":{"sql":"SELECT id FROM block_raw ORDER BY id"}}})
text = res["result"]["content"][0]["text"]
rows = json.loads(text)["rows"]
ids = sorted(r["id"] for r in rows)
print(f"[ios-reset] block_raw id-set ({len(ids)}):")
for i in ids: print(f"    {i}")
PY

echo "[ios-reset] done"
