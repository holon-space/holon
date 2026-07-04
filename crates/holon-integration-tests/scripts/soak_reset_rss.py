#!/usr/bin/env python3
"""Reset-memory soak: drive N reset_vault calls over one streamable-http session
and sample the app process RSS after each, to profile retained-engine growth.
Report-only for memory (the verdict is human); FAILS only if a reset errors or
the id-set drifts. Run against a freshly launched app (retire cap is 20/process,
so keep --count below the cap minus what the process already consumed)."""
import argparse, json, pathlib, subprocess, sys, time, urllib.request

SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
SEED_DIR = SCRIPT_DIR / "seed_wide"
SESSION = {"id": None}


def post(base, body):
    data = json.dumps(body).encode()
    req = urllib.request.Request(base, data=data, method="POST")
    req.add_header("Content-Type", "application/json")
    req.add_header("Accept", "application/json, text/event-stream")
    if SESSION["id"]:
        req.add_header("Mcp-Session-Id", SESSION["id"])
    resp = urllib.request.urlopen(req, timeout=120)
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


def tool(base, name, args, rid):
    res = post(base, {"jsonrpc": "2.0", "id": rid, "method": "tools/call",
                      "params": {"name": name, "arguments": args}})
    if res is None or "result" not in res:
        raise SystemExit(f"tool {name} failed: {json.dumps(res)}")
    return json.loads(res["result"]["content"][0]["text"])


def app_pid(bundle):
    out = subprocess.run(["pgrep", "-f", f"{bundle}|Holon.app/Holon"],
                         capture_output=True, text=True).stdout.split()
    if len(out) != 1:
        raise SystemExit(f"expected exactly one app pid, found: {out}")
    return int(out[0])


def rss_mb(pid):
    out = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)],
                         capture_output=True, text=True).stdout.strip()
    if not out:
        raise SystemExit(f"pid {pid} vanished mid-soak")
    return int(out) / 1024.0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--count", type=int, default=15)
    ap.add_argument("--port", type=int, default=8521)
    ap.add_argument("--pid", type=int, default=None)
    ap.add_argument("--bundle", default="space.holon.gpui")
    args = ap.parse_args()

    base = f"http://127.0.0.1:{args.port}/mcp"
    pid = args.pid or app_pid(args.bundle)
    files = [{"name": p.name, "content": p.read_text()}
             for p in sorted(SEED_DIR.glob("*.org"))]
    if not files:
        raise SystemExit(f"no seed files in {SEED_DIR}")

    post(base, {"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                           "clientInfo": {"name": "reset-soak", "version": "0.1"}}})
    notif = json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized",
                        "params": {}}).encode()
    req = urllib.request.Request(base, data=notif, method="POST")
    req.add_header("Content-Type", "application/json")
    req.add_header("Accept", "application/json, text/event-stream")
    req.add_header("Mcp-Session-Id", SESSION["id"])
    urllib.request.urlopen(req, timeout=10).read()

    print(f"pid={pid} baseline RSS={rss_mb(pid):.1f} MB")
    baseline_ids = None
    samples = []
    for i in range(1, args.count + 1):
        t0 = time.time()
        r = tool(base, "reset_vault", {"files": files}, 10 + i)
        dt = time.time() - t0
        ids = r["block_raw_ids"]
        if baseline_ids is None:
            baseline_ids = ids
        elif ids != baseline_ids:
            raise SystemExit(f"reset #{i}: id-set drifted from baseline")
        mb = rss_mb(pid)
        samples.append(mb)
        print(f"reset #{i:2d}: retired={r['retired_engines']:2d} "
              f"rows={r['block_raw_count']} reset_s={dt:.2f} rss={mb:.1f} MB")

    first, last, peak = samples[0], samples[-1], max(samples)
    per_reset = (last - first) / max(1, len(samples) - 1)
    print(f"\nRSS first={first:.1f} last={last:.1f} peak={peak:.1f} MB "
          f"(~{per_reset:.2f} MB/reset over {len(samples)} resets)")
    print("SOAK COMPLETE (memory verdict is yours; resets all deterministic)")


if __name__ == "__main__":
    main()
