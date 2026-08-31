#!/usr/bin/env python3
"""Saturation arm: fire N set_field ops back to back with no settle between.

Reproduces the ARRIVAL pattern the dogfood pass produced by accident (writes
entering the pipeline faster than the CDC delivery actor drains them), which is
what makes `stage="e2e"` — a dispatch->delivered wall clock — accumulate queue
wait on top of service time.
"""
import argparse
import json
import queue
import re
import sys
import threading
import time

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from mcp import Mcp  # noqa: E402
from measure_arms import read_e2e, stats  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    for f in ("port", "log", "block", "out", "tree"):
        ap.add_argument("--" + f, required=True)
    ap.add_argument("--n", type=int, default=32)
    ap.add_argument("--threads", type=int, default=8)
    a = ap.parse_args()

    conns = [Mcp(a.port) for _ in range(a.threads)]
    conns[0].call("await_quiescence", {})
    base = len(read_e2e(a.log))

    work = queue.Queue()
    for i in range(a.n):
        work.put(i)
    errs = []

    def run(c):
        while True:
            try:
                i = work.get_nowait()
            except queue.Empty:
                return
            try:
                c.call("type_text", {"text": "abcdefghij"[i % 10]})
            except Exception as e:  # recorded, never swallowed
                errs.append(f"{i}: {e}")

    t0 = time.time()
    ts = [threading.Thread(target=run, args=(c,)) for c in conns]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    wall = time.time() - t0
    conns[0].call("await_quiescence", {})
    time.sleep(3)

    vals = [e["ms"] for e in read_e2e(a.log)[base:] if e["action"] == "set_field"]
    res = {"tree": a.tree, "arm": "tight", "requested_n": a.n, "threads": a.threads,
           "wall_s": round(wall, 2), "errors": errs, "stats": stats(vals),
           "samples": vals}
    with open(a.out, "w") as fh:
        json.dump(res, fh, indent=2)
        fh.write("\n")
    print(json.dumps({k: v for k, v in res.items() if k != "samples"}, indent=2))


if __name__ == "__main__":
    main()
