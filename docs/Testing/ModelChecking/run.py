#!/usr/bin/env python3
"""Check every write-back and settle spec with TLC, and the Quint model.

Run from the repository root:  python3 docs/Testing/ModelChecking/run.py [--out DIR]

Each spec is checked once per shared property (SettleProperties.tla) and the
outcome is compared with EXPECTED; a spec with invariants only is checked per
INVARIANT_CHECKS, its witnesses included. The oracle of a red test is checked on the
design that closes its race (RED_TEST_ORACLES): a violation there means the
test cannot turn green after the fix, so it is no red-first test. Logs and `numbers.txt` (`key: value`
lines) go to DIR (default target/model-checking/logs).

Tools: TLA2TOOLS_JAR and QUINT name them; otherwise the jar is downloaded to
target/model-checking/ (checksum pinned) and Quint is installed there by npm.

Exit codes:
  0  every outcome is the expected one
  1  an outcome differs from EXPECTED (a green spec violates, a red one holds)
  2  no verdict: a tool failed (download, checksum, TLC or Quint error,
     unreadable log), the command line is wrong, or this script crashed
"""

import argparse
import hashlib
import os
import re
import subprocess
import sys
import time
import traceback
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
TOOLS = Path("target/model-checking")
JAR_URL = "https://github.com/tlaplus/tlaplus/releases/download/v1.7.4/tla2tools.jar"
JAR_SHA256 = "936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88"
QUINT_PACKAGE = "@informalsystems/quint@0.34.0"

INVARIANT = "SettledReadsNewest"
TEMPORAL = ["EveryOwedRenderWritten", "SettleReturns"]
PROPERTIES = [INVARIANT] + TEMPORAL

# spec (config name) -> the properties TLC must find violated
EXPECTED = {
    "Writeback_prefix": {INVARIANT},
    "Writeback_main": {INVARIANT},
    "Writeback_through": set(),
    "SettleWake_spin": set(),
    "SettleWake_fixed": set(),
    "SettleWake_nowake": {"SettleReturns"},
    "SettleWake_late": {"SettleReturns"},
}
# specs whose violating returns windows.py counts by pipeline stage
WINDOWS = ["Writeback_prefix", "Writeback_main"]
QUINT_EXPECTED_VIOLATED = "settledReadsNewest"
# config whose INVARIANT is a red test's oracle on the fixed design -> whether
# TLC must find that oracle violated
RED_TEST_ORACLES = {"Writeback_through_oracle": ("RedTestOracle", True)}
# config of a spec with invariants only -> invariant -> whether TLC must find
# it violated. A witness is expected violated: the state it excludes is
# reachable, so the property is not vacuous.
WITNESSES = [
    "QuietReadAfterChangeUnreached",
    "FoldNeverParked",
    "ChannelNeverHolds",
    "ConsumerNeverTakes",
    "CursorNeverApplied",
]
INVARIANT_CHECKS = {
    "HomeBySettle_flag": {INVARIANT: True},
    "HomeBySettle_through": {INVARIANT: False, **{w: True for w in WITNESSES}},
}

TLC_HOLDS, TLC_SAFETY, TLC_LIVENESS = 0, 12, 13


class ToolError(Exception):
    pass


def tla_jar() -> Path:
    if os.environ.get("TLA2TOOLS_JAR"):
        jar = Path(os.environ["TLA2TOOLS_JAR"])
        if not jar.is_file():
            raise ToolError(f"TLA2TOOLS_JAR={jar} is not a file")
        return jar
    jar = TOOLS / "tla2tools-1.7.4.jar"
    if not jar.is_file():
        TOOLS.mkdir(parents=True, exist_ok=True)
        with urllib.request.urlopen(JAR_URL) as response:
            data = response.read()
        digest = hashlib.sha256(data).hexdigest()
        if digest != JAR_SHA256:
            raise ToolError(f"{JAR_URL} has sha256 {digest}, expected {JAR_SHA256}")
        jar.write_bytes(data)
    return jar


def quint_bin() -> Path:
    if os.environ.get("QUINT"):
        quint = Path(os.environ["QUINT"])
        if not quint.is_file():
            raise ToolError(f"QUINT={quint} is not a file")
        return quint
    quint = TOOLS / "node_modules/.bin/quint"
    if not quint.is_file():
        TOOLS.mkdir(parents=True, exist_ok=True)
        done = subprocess.run(
            ["npm", "install", "--prefix", str(TOOLS), "--no-fund", "--no-audit", QUINT_PACKAGE]
        )
        if done.returncode != 0 or not quint.is_file():
            raise ToolError(f"npm install {QUINT_PACKAGE} failed ({done.returncode})")
    return quint


def number(pattern, text, log):
    found = re.findall(pattern, text)
    if not found:
        raise ToolError(f"{log}: no match for {pattern!r}")
    return int(found[-1].replace(",", ""))


def run_tlc(jar, spec, prop, out, extra=(), name=None):
    """TLC 1.7.4 reads a config only from the spec's own folder, so the specs
    are copied next to the generated config. prop None adds no property to
    the config."""
    module = spec.split("_")[0]
    specs = out / "specs"
    specs.mkdir(exist_ok=True)
    for tla in HERE.glob("*.tla"):
        (specs / tla.name).write_text(tla.read_text())
    base = (HERE / f"{spec}.cfg").read_text()
    name = name or prop or "graph"
    kind = "PROPERTY" if prop in TEMPORAL else "INVARIANT"
    cfg = f"{spec}-{name}.cfg"
    (specs / cfg).write_text(f"{base}{kind} {prop}\n" if prop else base)
    log = out / f"{spec}.{name}.log"
    # a fixed fingerprint function (-fp 0) keeps liveness traces reproducible
    cmd = [
        "nice", "java", "-XX:+UseParallelGC", "-cp", str(jar.resolve()), "tlc2.TLC",
        "-workers", "2", "-fp", "0", "-metadir", str((out / "states" / f"{spec}.{name}").resolve()),
        *extra, "-config", cfg, f"{module}.tla",
    ]
    started = time.monotonic()
    with open(log, "w") as f:
        f.write(f"CMD: {' '.join(cmd)}\n")
        f.flush()
        code = subprocess.run(cmd, stdout=f, stderr=subprocess.STDOUT, cwd=specs).returncode
    seconds = time.monotonic() - started
    return code, log, seconds


def check_tlc(jar, out, numbers):
    mismatches = []
    for spec, expected in EXPECTED.items():
        for prop in PROPERTIES:
            code, log, seconds = run_tlc(jar, spec, prop, out)
            text = log.read_text()
            violation_code = TLC_SAFETY if prop == INVARIANT else TLC_LIVENESS
            if code == TLC_HOLDS and "No error has been found" in text:
                violated = False
            elif code == violation_code and "violated" in text:
                violated = True
            else:
                raise ToolError(f"{log}: TLC exit {code} without a verdict")
            key = f"{spec}.{prop}"
            numbers[f"{key}.result"] = "violated" if violated else "holds"
            numbers[f"{key}.distinct_states"] = number(r"([\d,]+) distinct states found", text, log)
            numbers[f"{key}.states_generated"] = number(r"([\d,]+) states generated", text, log)
            numbers[f"{key}.seconds"] = f"{seconds:.1f}"
            if violated:
                numbers[f"{key}.trace_states"] = number(r"\nState (\d+):", text, log)
            print(f"{key}: {numbers[f'{key}.result']}, {numbers[f'{key}.distinct_states']} distinct states, "
                  f"{numbers[f'{key}.seconds']} s, log {log}", flush=True)
            if violated != (prop in expected):
                mismatches.append(f"{key}: {numbers[f'{key}.result']}, expected the other")
    return mismatches


def check_red_test_oracles(jar, out, numbers):
    mismatches = []
    for spec, (oracle, expect_violated) in RED_TEST_ORACLES.items():
        code, log, seconds = run_tlc(jar, spec, None, out, name=oracle)
        text = log.read_text()
        if code == TLC_HOLDS and "No error has been found" in text:
            violated = False
        elif code == TLC_SAFETY and f"Invariant {oracle} is violated" in text:
            violated = True
        else:
            raise ToolError(f"{log}: TLC exit {code} without a verdict")
        key = f"{spec}.{oracle}"
        numbers[f"{key}.result"] = "violated" if violated else "holds"
        numbers[f"{key}.distinct_states"] = number(r"([\d,]+) distinct states found", text, log)
        numbers[f"{key}.seconds"] = f"{seconds:.1f}"
        if violated:
            numbers[f"{key}.trace_states"] = number(r"\nState (\d+):", text, log)
        print(f"{key}: {numbers[f'{key}.result']}, log {log}", flush=True)
        if violated != expect_violated:
            mismatches.append(f"{key}: {numbers[f'{key}.result']}, expected the other")
    return mismatches


def check_invariants(jar, out, numbers):
    mismatches = []
    for spec, invariants in INVARIANT_CHECKS.items():
        for inv, expect_violated in invariants.items():
            code, log, seconds = run_tlc(jar, spec, inv, out)
            text = log.read_text()
            if code == TLC_HOLDS and "No error has been found" in text:
                violated = False
            elif code == TLC_SAFETY and f"Invariant {inv} is violated" in text:
                violated = True
            else:
                raise ToolError(f"{log}: TLC exit {code} without a verdict")
            key = f"{spec}.{inv}"
            numbers[f"{key}.result"] = "violated" if violated else "holds"
            numbers[f"{key}.distinct_states"] = number(r"([\d,]+) distinct states found", text, log)
            numbers[f"{key}.seconds"] = f"{seconds:.1f}"
            if violated:
                numbers[f"{key}.trace_states"] = number(r"\nState (\d+):", text, log)
            print(f"{key}: {numbers[f'{key}.result']}, log {log}", flush=True)
            if violated != expect_violated:
                mismatches.append(f"{key}: {numbers[f'{key}.result']}, expected the other")
    return mismatches


def check_windows(jar, out, numbers):
    for spec in WINDOWS:
        graph = (out / f"{spec}.dot").resolve()
        code, log, seconds = run_tlc(jar, spec, None, out, extra=("-dump", "dot,actionlabels", str(graph)))
        if code != TLC_HOLDS or "No error has been found" not in log.read_text():
            raise ToolError(f"{log}: TLC state graph dump exit {code}")
        numbers[f"{spec}.graph.seconds"] = f"{seconds:.1f}"
        done = subprocess.run(
            [sys.executable, str(HERE / "windows.py"), str(graph), f"{spec}.window"],
            capture_output=True, text=True,
        )
        if done.returncode != 0:
            raise ToolError(f"windows.py {log}: {done.stderr.strip()}")
        for line in done.stdout.splitlines():
            key, value = line.split(": ")
            numbers[key] = value


def check_quint(quint, out, numbers):
    log = out / "Writeback.qnt.log"
    # a fixed fingerprint function (-fp 0) keeps liveness traces reproducible
    cmd = [
        str(quint), "run", "--invariant", QUINT_EXPECTED_VIOLATED, "--max-samples", "10000",
        "--max-steps", "30", "--seed", "1", "--verbosity", "3", str(HERE / "Writeback.qnt"),
    ]
    started = time.monotonic()
    with open(log, "w") as f:
        f.write(f"CMD: {' '.join(cmd)}\n")
        f.flush()
        code = subprocess.run(cmd, stdout=f, stderr=subprocess.STDOUT).returncode
    numbers["quint.Writeback.seconds"] = f"{time.monotonic() - started:.1f}"
    text = log.read_text()
    if code == 0 and "No violation found" in text:
        numbers["quint.Writeback.result"] = "holds"
        return [f"quint Writeback.{QUINT_EXPECTED_VIOLATED}: holds, expected violated"]
    if code != 0 and f"❌ {QUINT_EXPECTED_VIOLATED}" in text:
        numbers["quint.Writeback.result"] = "violated"
        numbers["quint.Writeback.trace_states"] = number(r"\[State (\d+)\]", text, log) + 1
        return []
    raise ToolError(f"{log}: quint exit {code} without a verdict")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", default=str(TOOLS / "logs"))
    out = Path(parser.parse_args().out)
    (out / "states").mkdir(parents=True, exist_ok=True)
    numbers = {}
    try:
        mismatches = check_tlc(tla_jar(), out, numbers)
        mismatches += check_red_test_oracles(tla_jar(), out, numbers)
        mismatches += check_invariants(tla_jar(), out, numbers)
        check_windows(tla_jar(), out, numbers)
        mismatches += check_quint(quint_bin(), out, numbers)
    except Exception as e:
        print(f"TOOL ERROR: {type(e).__name__}: {e}", file=sys.stderr)
        sys.exit(2)
    finally:
        (out / "numbers.txt").write_text("".join(f"{k}: {v}\n" for k, v in numbers.items()))
    for m in mismatches:
        print(f"MISMATCH: {m}")
    print(f"numbers: {out / 'numbers.txt'}")
    sys.exit(1 if mismatches else 0)


if __name__ == "__main__":
    try:
        main()
    except Exception:
        traceback.print_exc()
        sys.exit(2)
