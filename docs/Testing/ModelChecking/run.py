#!/usr/bin/env python3
"""Check every write-back, settle and work-runtime spec with TLC, and the Quint model.

Run from the repository root:  python3 docs/Testing/ModelChecking/run.py [--out DIR] [--if-touched FILE]

With --if-touched FILE (changed paths, one per line) the specs run only when a
path is a file named by a `Models:` comment of a spec or lies in this folder;
otherwise it prints `SKIP: no modelled file touched` and exits 0.

Each spec is checked once per shared property (SettleProperties.tla) and the
outcome is compared with EXPECTED. The oracle of a red test is checked on the
design that closes its race (RED_TEST_ORACLES): a violation there means the
test cannot turn green after the fix, so it is no red-first test. Logs and `numbers.txt` (`key: value`
lines) go to DIR (default target/model-checking/logs).

Tools: TLA2TOOLS_JAR and QUINT name them; otherwise the jar is downloaded to
target/model-checking/ (checksum pinned) and Quint is installed there by npm.

Exit codes:
  0  every outcome is the expected one
  1  an outcome differs from EXPECTED (a green spec violates, a red one holds)
  2  no verdict: a tool failed (missing java or jar, download, checksum, TLC or
     Quint error, unreadable log), a `Models:` anchor names a missing file or
     function, the changed-paths file is unreadable or empty, the command line
     is wrong, or this script crashed
"""

from __future__ import annotations

import argparse
import hashlib
import os
import re
import shutil
import subprocess
import sys
import time
import traceback
import urllib.request
from pathlib import Path
from typing import NamedTuple

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
# work-runtime config -> property -> (TLC kind, whether TLC must find it
# violated). WorkRuntimeNet.tla is generated from the net file; every
# property that holds has a witness row that must be violated.
WORK_RUNTIME = {
    "WorkRuntime_exec": {
        "AtMostPoolJobs": ("INVARIANT", False),
        "NotAllSlotsBusy": ("INVARIANT", True),
        "NeverJobAndWaiting": ("INVARIANT", True),
        "SlotsConserved": ("INVARIANT", True),
        "JobEndReturnsSlot": ("PROPERTY", True),
        "NoJobEnds": ("PROPERTY", True),
    },
    "WorkRuntime_l0": {
        "NoJobWhileStarting": ("INVARIANT", False),
        "JobEndReturnsSlot": ("PROPERTY", False),
    },
}
# specs whose violating returns windows.py counts by pipeline stage
WINDOWS = ["Writeback_prefix", "Writeback_main"]
QUINT_EXPECTED_VIOLATED = "settledReadsNewest"
# config whose INVARIANT is a red test's oracle on the fixed design -> whether
# TLC must find that oracle violated
RED_TEST_ORACLES = {"Writeback_through_oracle": ("RedTestOracle", True)}

TLC_HOLDS, TLC_SAFETY, TLC_LIVENESS = 0, 12, 13


class ToolError(Exception):
    pass


class Anchor(NamedTuple):
    path: str
    function: str
    spec: str
    line: int


MODELS_ANCHOR = re.compile(r"(?<![\w./-])([\w./-]+\.rs):(\w+(?:::\w+)*)")
SKIP_MESSAGE = "SKIP: no modelled file touched"


def modelled_files(specs_dir: Path) -> set[Anchor]:
    """The `path:function` anchors of every `Models:` comment in the specs: the
    comment from the `Models:` line to the end of its block of `\\*` lines. Raises
    ToolError for an anchor whose file or function is missing, naming the spec line."""
    anchors = set()
    for tla in sorted(specs_dir.glob("*.tla")):
        lines = tla.read_text().splitlines()
        for i, line in enumerate(lines):
            if not line.lstrip().startswith("\\*") or "Models:" not in line:
                continue
            end = i + 1
            while end < len(lines) and lines[end].lstrip().startswith("\\*"):
                end += 1
            found = MODELS_ANCHOR.findall(line.split("Models:", 1)[1] + "\n" + "\n".join(lines[i + 1:end]))
            if not found:
                raise ToolError(f"{tla.name}:{i + 1}: `Models:` names no <file>.rs:<function>")
            for path, function in found:
                anchors.add(Anchor(path, function, tla.name, i + 1))
    for a in anchors:
        where = f"{a.spec}:{a.line}: {a.path}:{a.function}"
        if not Path(a.path).is_file():
            raise ToolError(f"{where}: file {a.path} does not exist")
        name = a.function.split("::")[-1]
        if not re.search(rf"\bfn\s+{name}\b", Path(a.path).read_text()):
            raise ToolError(f"{where}: no `fn {name}` in {a.path}")
    return anchors


def touch_reason(changed_file: Path) -> str:
    """`RUN: <why>` when the specs must run for these changed paths, else a `SKIP: <why>` message.
    A missing or unreadable list is an error; a readable list without a path is a clean tree."""
    try:
        changed = [p for p in changed_file.read_text().splitlines() if p.strip()]
    except OSError as e:
        raise ToolError(f"changed-paths file {changed_file}: {e}")
    if not changed:
        return "SKIP: no changed path"
    anchors = modelled_files(HERE)
    folder = HERE.relative_to(Path.cwd().resolve()).as_posix() + "/"
    for path in changed:
        if path.startswith(folder):
            return f"RUN: {path} is in {folder}"
        for a in sorted(anchors):
            if a.path == path:
                return f"RUN: {path} is modelled by {a.spec}:{a.line} ({a.function})"
    return SKIP_MESSAGE


def require_java():
    if shutil.which("java") is None:
        raise ToolError(f"java not found on PATH={os.environ.get('PATH', '')}")


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


def run_tlc(jar, spec, prop, out, extra=(), name=None, kind=None):
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
    kind = kind or ("INVARIANT" if prop == INVARIANT else "PROPERTY")
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


def check_work_runtime(jar, out, numbers):
    mismatches = []
    for spec, props in WORK_RUNTIME.items():
        for prop, (kind, expect_violated) in props.items():
            code, log, seconds = run_tlc(jar, spec, prop, out, kind=kind)
            text = log.read_text()
            violation_code = TLC_SAFETY if kind == "INVARIANT" else TLC_LIVENESS
            if code == TLC_HOLDS and "No error has been found" in text:
                violated = False
            elif code == violation_code and f"{prop} is violated" in text:
                violated = True
            else:
                raise ToolError(f"{log}: TLC exit {code} without a verdict")
            key = f"{spec}.{prop}"
            numbers[f"{key}.result"] = "violated" if violated else "holds"
            numbers[f"{key}.distinct_states"] = number(r"([\d,]+) distinct states found", text, log)
            numbers[f"{key}.seconds"] = f"{seconds:.1f}"
            if violated:
                numbers[f"{key}.trace_states"] = number(r"\nState (\d+):", text, log)
            print(f"{key}: {numbers[f'{key}.result']}, {numbers[f'{key}.distinct_states']} distinct states, "
                  f"{numbers[f'{key}.seconds']} s, log {log}", flush=True)
            if violated != expect_violated:
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
    parser.add_argument("--if-touched", metavar="FILE", help="changed paths, one per line; run only when one is modelled")
    args = parser.parse_args()
    if args.if_touched:
        try:
            reason = touch_reason(Path(args.if_touched))
        except ToolError as e:
            print(f"TOOL ERROR: {e}", file=sys.stderr)
            sys.exit(2)
        print(reason)
        if reason.startswith("SKIP:"):
            sys.exit(0)
    out = Path(args.out)
    (out / "states").mkdir(parents=True, exist_ok=True)
    numbers = {}
    try:
        require_java()
        mismatches = check_tlc(tla_jar(), out, numbers)
        mismatches += check_red_test_oracles(tla_jar(), out, numbers)
        mismatches += check_work_runtime(tla_jar(), out, numbers)
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
