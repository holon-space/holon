#!/usr/bin/env python3
"""Drive N set_field interactions in two arms and report e2e latency per arm.

  burst  — one type_text of N characters, exactly what the dogfood pass did:
           N dispatches enter the pipeline back to back.
  paced  — N separate type_text calls, each followed by await_quiescence, so
           at most one interaction is ever in flight.

Both arms read the SAME app log, so the two populations differ only in arrival
pattern. Emits a machine-readable summary to stdout and a table to the file
named by --out.
"""
import argparse
import json
import re
import sys
import time

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from mcp import Mcp  # noqa: E402

ANSI = re.compile(r"\x1b\[[0-9;]*m")
E2E = re.compile(r'stage="e2e" action=(\w+) block=(\S+) source="(\w+)" ms=(\d+)')


def read_e2e(log):
    out = []
    with open(log, errors="replace") as fh:
        for line in fh:
            m = E2E.search(ANSI.sub("", line))
            if m:
                out.append({"action": m.group(1), "block": m.group(2),
                            "source": m.group(3), "ms": int(m.group(4))})
    return out


def inactive_count(log):
    """WINDOW-INACTIVE markers the app itself logs — the run's window-state
    certificate. Latency measured while the render loop is OS-throttled is not
    comparable to latency measured on an active window."""
    with open(log, errors="replace") as fh:
        return sum(1 for line in fh if "WINDOW-INACTIVE" in line)


def pct(vals, p):
    if not vals:
        return float("nan")
    s = sorted(vals)
    if len(s) == 1:
        return float(s[0])
    k = (len(s) - 1) * p / 100.0
    lo, hi = int(k), min(int(k) + 1, len(s) - 1)
    return s[lo] + (s[hi] - s[lo]) * (k - lo)


def stats(vals):
    return {"n": len(vals), "p50": round(pct(vals, 50), 1), "p95": round(pct(vals, 95), 1),
            "max": float(max(vals)) if vals else float("nan"),
            "mean": round(sum(vals) / len(vals), 1) if vals else float("nan")}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", required=True)
    ap.add_argument("--log", required=True)
    ap.add_argument("--block", required=True)
    ap.add_argument("--n", type=int, default=32)
    ap.add_argument("--out", required=True)
    ap.add_argument("--tree", required=True)
    a = ap.parse_args()

    m = Mcp(a.port)
    m.call("click", {"entity_id": a.block})
    m.call("await_quiescence", {})
    base = len(read_e2e(a.log))
    inact0 = inactive_count(a.log)

    burst_text = "".join("abcdefghij"[i % 10] for i in range(a.n))
    t0 = time.time()
    m.call("type_text", {"text": burst_text})
    burst_wall = time.time() - t0
    m.call("await_quiescence", {})
    time.sleep(2)
    after_burst = read_e2e(a.log)
    burst = [e["ms"] for e in after_burst[base:] if e["action"] == "set_field"]
    inact1 = inactive_count(a.log)

    paced = []
    mark = len(after_burst)
    t0 = time.time()
    for i in range(a.n):
        m.call("type_text", {"text": "abcdefghij"[i % 10]})
        m.call("await_quiescence", {})
    paced_wall = time.time() - t0
    time.sleep(2)
    allev = read_e2e(a.log)
    paced = [e["ms"] for e in allev[mark:] if e["action"] == "set_field"]

    res = {"tree": a.tree, "port": a.port, "log": a.log, "block": a.block,
           "requested_n": a.n,
           "burst": stats(burst), "burst_wall_s": round(burst_wall, 2),
           "burst_samples": burst, "burst_window_inactive": inact1 - inact0,
           "paced": stats(paced), "paced_wall_s": round(paced_wall, 2),
           "paced_samples": paced,
           "paced_window_inactive": inactive_count(a.log) - inact1}
    with open(a.out, "w") as fh:
        json.dump(res, fh, indent=2)
        fh.write("\n")
    print(json.dumps({k: v for k, v in res.items() if not k.endswith("samples")}, indent=2))


if __name__ == "__main__":
    main()
