#!/usr/bin/env python3
"""Run merged test binaries in ONE process, in drawn test orders, and fail on any red.

    scripts/test-merged-binaries-one-process.py [--seed S] [--draws N] [--target NAME]...

A test binary that holds many modules is green only if no test leaves process-wide state behind
(tracing default, panic hook, env var, panic-record target). nextest runs one process per test and
hides such a leak; `cargo test` runs one process and shows it. Property: for every drawn test order the
binary is green.

Each draw is a process run: `--test-threads=<t> -Z unstable-options --shuffle-seed <s>`, with t drawn
from {1, 2, 8}: concurrent tests share the process too. Seed and thread count are printed; a red draw is
replayed with `--seed <S> --draws 1 --target <name>`.
Static examples keep their order as an explicit list of test-name prefixes: the script draws seeds until the
binary ran those tests in that order, and fails if no drawn seed produces it.

The test binaries come from one `cargo test --no-run` of the canonical compile shape; they run with the
working directory and CARGO_MANIFEST_DIR of their package, as cargo runs them.
"""
import argparse
import json
import os
import random
import re
import subprocess
import sys

REPO = os.path.dirname(os.path.dirname(os.path.realpath(__file__)))
SHAPE_ENV = os.path.join(REPO, "scripts", "cargo-shape-env.sh")
CANON = ["--workspace", "--features", "holon-integration-tests/pbt,holon-integration-tests/web-arm,holon-gpui/pbt"]

WARM = "warm_reboot_keeps_matviews::"
STUCK = "stuck_actor_watch::"
VALUE_FN = "value_fn_panic_degrades_only_its_widget::"
EVERY = "every_builder_survives_authored_args::"
HAND = "hand_authored_regressions::"

# name -> (package, cargo target kind, target name, filters, env, static orders). A static order lists test-name
# prefixes; the binary must have run a test of each prefix, in that order.
TARGETS = {
    "turso-main": ("holon-turso", "test", "main", [], {}, [[WARM, STUCK], [STUCK, WARM]]),
    "turso-main-boots": ("holon-turso", "test", "main", [
        WARM, "previous_engine_database_boots::", "views_of_another_engine_build::", "matview_point_read_index::"],
        {}, []),
    "turso-lib": ("holon-turso", "lib", None, [], {}, []),
    "frontend-main": ("holon-frontend", "test", "main", [], {}, [[VALUE_FN, EVERY], [EVERY, VALUE_FN]]),
    "hand-authored-shard0": (
        "holon-integration-tests", "test", "integration", [HAND], {"HOLON_HAND_AUTHORED_SHARD": "0/4"}, []),
    "hand-authored-refusals": (
        "holon-integration-tests", "test", "hand_authored_refusals", [], {}, []),
    "holon": ("holon", "test", None, [], {}, []),
    "holon-app": ("holon-app", "test", None, [], {}, []),
}
# Targets of the default run: the binaries the lane keeps green. `holon` and `holon-app` are report-only.
DEFAULT = ["turso-main", "turso-main-boots", "turso-lib", "frontend-main", "hand-authored-shard0", "hand-authored-refusals"]


def build():
    """{(package dir, kind, target): [executable]} from one canonical `cargo test --no-run`."""
    cmd = "source %s lane && cargo test %s --no-run --message-format=json" % (SHAPE_ENV, " ".join(CANON))
    out = subprocess.run(["bash", "-c", cmd], cwd=REPO, stdout=subprocess.PIPE, text=True)
    if out.returncode != 0:
        sys.exit("build failed (exit %d)" % out.returncode)
    exes = {}
    for line in out.stdout.splitlines():
        msg = json.loads(line)
        if msg.get("reason") != "compiler-artifact" or not msg.get("profile", {}).get("test") or not msg.get("executable"):
            continue
        pkg = os.path.dirname(msg["manifest_path"])
        kinds = msg["target"]["kind"]
        kind = "lib" if "lib" in kinds else "test" if "test" in kinds else "bin"
        exes.setdefault((pkg, kind, msg["target"]["name"]), []).append(msg["executable"])
    return exes


def package_dirs():
    meta = json.loads(subprocess.run(["cargo", "metadata", "--no-deps", "--format-version", "1"], cwd=REPO,
                                     stdout=subprocess.PIPE, text=True, check=True).stdout)
    return {p["name"]: os.path.dirname(p["manifest_path"]) for p in meta["packages"]}


def run(exe, pkgdir, filters, env, seed, threads=1):
    full_env = dict(os.environ, CARGO_MANIFEST_DIR=pkgdir, **env)
    argv = [exe, "--test-threads=%d" % threads, "-Z", "unstable-options", "--shuffle-seed", str(seed), "--nocapture", *filters]
    res = subprocess.run(argv, cwd=pkgdir, env=full_env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    return res.returncode, res.stdout


def order_of(output):
    return [m.group(1) for m in re.finditer(r"^test (\S+) \.\.\. ", output, re.M)]


def has_order(names, prefixes):
    pos = -1
    for p in prefixes:
        hit = next((i for i, n in enumerate(names) if i > pos and n.startswith(p)), None)
        if hit is None:
            return False
        pos = hit
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seed", type=int, default=random.SystemRandom().randrange(2**32))
    ap.add_argument("--draws", type=int, default=3)
    ap.add_argument("--target", action="append", choices=sorted(TARGETS))
    ap.add_argument("--max-order-seeds", type=int, default=32)
    args = ap.parse_args()
    print("SEED=%d DRAWS=%d" % (args.seed, args.draws), flush=True)
    exes = build()
    dirs = package_dirs()
    rng = random.Random(args.seed)
    failures = []
    for name in args.target or DEFAULT:
        pkg, kind, tname, filters, env, orders = TARGETS[name]
        pkgdir = dirs[pkg]
        found = [e for (p, k, t), es in exes.items() if p == pkgdir and k == kind and (tname is None or t == tname) for e in es]
        if not found:
            failures.append("%s: no test binary %s %s %s" % (name, pkg, kind, tname))
            continue
        for exe in found:
            for seed in [rng.randrange(2**32) for _ in range(args.draws)]:
                threads = rng.choice([1, 2, 8])
                code, out = run(exe, pkgdir, filters, env, seed, threads)
                print("%s %s shuffle-seed=%d threads=%d exit=%d" % (name, os.path.basename(exe), seed, threads, code), flush=True)
                if code != 0:
                    failures.append("%s shuffle-seed=%d threads=%d" % (name, seed, threads))
                    print(out[-4000:])
            for prefixes in orders:
                for seed in range(args.max_order_seeds):
                    code, out = run(exe, pkgdir, prefixes, env, seed)
                    if not has_order(order_of(out), prefixes):
                        continue
                    print("%s order %s exit=%d (shuffle-seed=%d)" % (name, prefixes, code, seed), flush=True)
                    if code != 0:
                        failures.append("%s order %s" % (name, prefixes))
                        print(out[-4000:])
                    break
                else:
                    failures.append("%s order %s never drawn in %d seeds" % (name, prefixes, args.max_order_seeds))
    if failures:
        print("RED: " + "; ".join(failures))
        sys.exit(1)
    print("GREEN")


if __name__ == "__main__":
    main()
