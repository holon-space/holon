#!/usr/bin/env python3
"""Phase 1 Option A verification: drive reset_vault twice over ONE streamable-http
session and assert the reset is deterministic, the same session observes the
swapped engine (C2), and the retirement list grows per reset."""
import json, sys, urllib.request, pathlib

BASE = "http://127.0.0.1:8521/mcp"
SESSION = {"id": None}
SEED_DIR = pathlib.Path(
    "/Users/martin/Workspaces/pkm/holon/.claude/worktrees/ios-keyboard-crash-fix/"
    "crates/holon-integration-tests/scripts/seed_wide"
)


def post(body):
    data = json.dumps(body).encode()
    req = urllib.request.Request(BASE, data=data, method="POST")
    req.add_header("Content-Type", "application/json")
    req.add_header("Accept", "application/json, text/event-stream")
    if SESSION["id"]:
        req.add_header("Mcp-Session-Id", SESSION["id"])
    resp = urllib.request.urlopen(req, timeout=60)
    sid = resp.headers.get("Mcp-Session-Id")
    if sid:
        SESSION["id"] = sid
    out = None
    for line in resp.read().decode().splitlines():
        line = line.strip()
        if line.startswith("data:"):
            out = json.loads(line[5:].strip())
        elif line.startswith("{"):
            out = json.loads(line)
    return out


def rpc(method, params=None, rid=1):
    return post({"jsonrpc": "2.0", "id": rid, "method": method, "params": params or {}})


def notify(method, params=None):
    data = json.dumps({"jsonrpc": "2.0", "method": method, "params": params or {}}).encode()
    req = urllib.request.Request(BASE, data=data, method="POST")
    req.add_header("Content-Type", "application/json")
    req.add_header("Accept", "application/json, text/event-stream")
    if SESSION["id"]:
        req.add_header("Mcp-Session-Id", SESSION["id"])
    urllib.request.urlopen(req, timeout=10).read()


def tool(name, args, rid):
    res = rpc("tools/call", {"name": name, "arguments": args}, rid=rid)
    if res is None or "result" not in res:
        raise SystemExit(f"tool {name} failed: {json.dumps(res)}")
    return json.loads(res["result"]["content"][0]["text"])


def seed_files():
    return [
        {"name": p.name, "content": p.read_text()}
        for p in sorted(SEED_DIR.glob("*.org"))
    ]


# handshake — ONE session for the whole run
rpc("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                   "clientInfo": {"name": "reset-verify", "version": "0.1"}})
notify("notifications/initialized")
print("SESSION:", SESSION["id"])

files = seed_files()
print("seed files:", [f["name"] for f in files])

r1 = tool("reset_vault", {"files": files}, 2)
print("reset#1:", json.dumps(r1))
# same-session read AFTER reset — the C2 check
q1 = tool("execute_raw_sql", {"sql": "SELECT id FROM block_raw ORDER BY id"}, 3)
ids_after_1 = [row.get("id") for row in q1.get("rows", [])]

r2 = tool("reset_vault", {"files": files}, 4)
print("reset#2:", json.dumps(r2))
q2 = tool("execute_raw_sql", {"sql": "SELECT id FROM block_raw ORDER BY id"}, 5)
ids_after_2 = [row.get("id") for row in q2.get("rows", [])]

set1, set2 = r1["block_raw_ids"], r2["block_raw_ids"]
print(f"\nreset#1 count={r1['block_raw_count']} retired={r1['retired_engines']}")
print(f"reset#2 count={r2['block_raw_count']} retired={r2['retired_engines']}")
print(f"same-session read after #1: {len(ids_after_1)} ids")
print(f"same-session read after #2: {len(ids_after_2)} ids")

ok = True
if set1 != set2:
    ok = False; print("FAIL: id-set not deterministic across resets")
if r2["retired_engines"] != r1["retired_engines"] + 1:
    ok = False; print("FAIL: retirement list did not grow by exactly 1")
if len(ids_after_2) != r2["block_raw_count"]:
    ok = False; print("FAIL: same-session post-reset read disagrees with self-check (C2 stale!)")
if r1["block_raw_count"] == 0:
    ok = False; print("FAIL: reset produced an empty vault")

print("\nRESULT:", "PASS" if ok else "FAIL")
sys.exit(0 if ok else 1)
