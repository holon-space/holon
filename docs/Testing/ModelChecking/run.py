#!/usr/bin/env python3
"""Check every write-back, settle and reader-isolation spec with TLC, and the Quint model.

Run from the repository root:  python3 docs/Testing/ModelChecking/run.py [--out DIR]

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

TLC_HOLDS, TLC_SAFETY, TLC_LIVENESS = 0, 12, 13

# Reader isolation during a take (ReaderIsolation*.tla). A row:
# (label, cfg, Case or None, kind, names, expected), where expected is
# "holds" or the text TLC prints for the violation it must find.
RI_INVARIANTS = ["TypeOK", "I1Exclusive", "I2Dependency", "I3CommitWait", "I4TakeCommits",
                 "I5TakeFails", "I6Sound", "FactSound", "I7Disclosure", "I7NoStaleCondition"]
RI_LIVENESS = ["ReleasedUnlessParked", "ReleasedWhenResolved", "QuiesceEnds", "StaleReleased"]
RI_CASES = {
    "held_token_superseded_by_a_write": "case_reader",
    "absent_read_then_written_and_taken": "case_reader",
    "dirty_absent_read_records_before_the_taker_fails": "case_reader",
    "read_of_a_token_written_during_a_take": "case_reader",
    "run_state_read_then_the_run_starts": "case_running",
    "fact_into_fifo_appended_behind_head": "case_fifo_reader_fact",
    "fifo_append_behind_the_read_head": "case_fifo_reader",
    "read_then_taken_taker_runs": "case_reader",
    "read_then_taken_taker_done": "case_reader",
    "read_then_taken_taker_fails": "case_reader",
    "read_while_taken_taker_done": "case_reader",
    "read_while_taken_taker_fails": "case_reader",
    "read_while_taken_taker_retries": "case_reader",
    "present_fifo_read_whose_held_front_goes_back": "case_fifo",
    "a_stale_reader_puts_no_fact_over_the_restored_token": "case_fact",
    "reader_waits_on_a_parked_take_verify": "case_reader",
    "reader_waits_on_a_parked_take_drop": "case_reader",
    "reader_waits_on_a_parked_take_retry_then_fails": "case_reader",
}


def violated(name):
    return f"Invariant {name} is violated"


TEMPORAL_VIOLATED = "Temporal properties were violated"
RI_ROWS = (
    [(f"{d}.invariants", f"ReaderIsolation_{d}", None, "INVARIANT", RI_INVARIANTS, "holds")
     for d in ["single", "register", "fifo", "fact", "purefact", "tworows", "twosubjects", "wide"]]
    + [(f"{d}.{p}", f"ReaderIsolation_{d}", None, "PROPERTY", [p], "holds")
       for d in ["singlelive", "registerlive", "fifolive", "factlive"] for p in RI_LIVENESS]
    + [(f"{d}.{w}", f"ReaderIsolation_{d}", None, "INVARIANT", [w], violated(w))
       for d, ws in [
           ("single", ["NeverHeldToken", "NeverEdge", "NeverHeldReader", "NeverDoneAfterItsTakeCommitted",
                       "NeverStaleByRestore", "NeverDoneRecord", "NeverWaitsParked", "NeverCondition",
                       "NeverHeldStale"]),
           ("fifo", ["NeverEdge", "NeverStaleByRestore"]),
           ("fact", ["NeverFactRefused", "NeverFactLanded"]),
           ("tworows", ["NeverStaleByRestore", "NeverFactRefused"]),
           ("twosubjects", ["NeverWaitsParked"]),
       ] for w in ws]
    + [
        ("singlelive.ReleasedWithoutResolve", "ReaderIsolation_singlelive", None, "PROPERTY",
         ["ReleasedWithoutResolve"], TEMPORAL_VIOLATED),
        ("oldcond.I7NoStaleCondition", "ReaderIsolation_oldcond", None, "INVARIANT",
         ["I7NoStaleCondition"], violated("I7NoStaleCondition")),
        ("holdfirstlive.StaleReleased", "ReaderIsolation_holdfirstlive", None, "PROPERTY",
         ["StaleReleased"], TEMPORAL_VIOLATED),
        ("staleatrecordlive.StaleReleased", "ReaderIsolation_staleatrecordlive", None, "PROPERTY",
         ["StaleReleased"], TEMPORAL_VIOLATED),
        ("nowait.I6Sound", "ReaderIsolation_nowait", None, "INVARIANT", ["I6Sound"], violated("I6Sound")),
        ("nostale.I5TakeFails", "ReaderIsolation_nostale", None, "INVARIANT", ["I5TakeFails"],
         violated("I5TakeFails")),
        ("nostale.I6Sound", "ReaderIsolation_nostale", None, "INVARIANT", ["I6Sound"], violated("I6Sound")),
        ("noadvance.I4TakeCommits", "ReaderIsolation_noadvance", None, "INVARIANT", ["I4TakeCommits"],
         violated("I4TakeCommits")),
        ("keepedge.deadlock", "ReaderIsolation_keepedge", None, "INVARIANT", ["TypeOK"], "Deadlock reached"),
        ("keepedgelive.ReleasedUnlessParked", "ReaderIsolation_keepedgelive", None, "PROPERTY",
         ["ReleasedUnlessParked"], TEMPORAL_VIOLATED),
        ("nocond.I7Disclosure", "ReaderIsolation_nocond", None, "INVARIANT", ["I7Disclosure"],
         violated("I7Disclosure")),
        ("oldtrace.QuiesceEnds", "ReaderIsolation_oldtrace", None, "PROPERTY", ["QuiesceEnds"],
         TEMPORAL_VIOLATED),
        ("answers.production", "ReaderIsolationAnswers_production", None, "INVARIANT",
         ["OneEffectPerAsk"], "holds"),
        ("answers.production.NoAskAnswered", "ReaderIsolationAnswers_production", None, "INVARIANT",
         ["NoAskAnswered"], violated("NoAskAnswered")),
        ("answers.testnet", "ReaderIsolationAnswers_testnet", None, "INVARIANT",
         ["OneEffectPerAsk"], violated("OneEffectPerAsk")),
        ("case.read_of_a_token_written_during_a_take.wide", "ReaderIsolation_case_reader_wide",
         "read_of_a_token_written_during_a_take", "INVARIANT", ["CaseHolds"], violated("CaseHolds")),
    ]
    + [(f"case.{c}", f"ReaderIsolation_{net}", c, "INVARIANT", ["CaseHolds"] + RI_INVARIANTS[3:], "holds")
       for c, net in RI_CASES.items()]
    + [(f"case.{c}.end", f"ReaderIsolation_{net}", c, "INVARIANT", ["ScriptNeverEnds"],
        violated("ScriptNeverEnds"))
       for c, net in RI_CASES.items()]
    + [
        ("supersede.version", "ReaderIsolationSupersede_version", "free", "INVARIANT",
         ["NoRevive", "I6Sound"], "holds"),
        ("supersede.slot", "ReaderIsolationSupersede_slot", "free", "INVARIANT", ["NoRevive"],
         violated("NoRevive")),
        ("supersede.slot.I6Sound", "ReaderIsolationSupersede_slot", "free", "INVARIANT", ["I6Sound"],
         violated("I6Sound")),
        ("case.superseded_token_stays_out_after_the_newer_one_is_consumed.slot",
         "ReaderIsolationSupersede_slot", "superseded_token_stays_out_after_the_newer_one_is_consumed",
         "INVARIANT", ["CaseHolds"], violated("CaseHolds")),
    ]
    + [(f"supersede.version.{w}", "ReaderIsolationSupersede_version", "free", "INVARIANT", [w], violated(w))
       for w in ["NeverRestored", "NeverStale", "NeverNotStale"]]
    + [(f"case.{c}{end}", "ReaderIsolationSupersede_version", c, "INVARIANT", props, expected)
       for c in ["superseded_token_stays_out_when_a_later_take_holds_the_newer_one",
                 "superseded_token_stays_out_after_the_newer_one_is_consumed"]
       for end, props, expected in [("", ["CaseHolds", "NoRevive", "I6Sound"], "holds"),
                                    (".end", ["ScriptNeverEnds"], violated("ScriptNeverEnds"))]]
    + [(f"idle.{d}", f"ReaderIsolationIdle_{d}", None, "INVARIANT", ["IdleRule", "I6Sound"], "holds")
       for d in ["narrow_off", "narrow_on", "absent_on"]]
    + [(f"idle.{d}.IdleRule", f"ReaderIsolationIdle_{d}", None, "INVARIANT", ["IdleRule"], violated("IdleRule"))
       for d in ["wide_off", "wide_on", "absent_off"]]
    + [(f"idle.{d}.I6Sound", f"ReaderIsolationIdle_{d}", None, "INVARIANT", ["I6Sound"], "holds")
       for d in ["wide_off", "wide_on", "absent_off"]]
    + [(f"idle.{d}.NeverAnswered", f"ReaderIsolationIdle_{d}", None, "INVARIANT", ["NeverAnswered"],
        violated("NeverAnswered"))
       for d in ["narrow_on", "absent_on"]]
    + [
        ("fifoask.d", "ReaderIsolationFifoAsk_d", None, "INVARIANT", ["AskConsumesWhatItSaw"], "holds"),
        ("fifoask.current", "ReaderIsolationFifoAsk_current", None, "INVARIANT", ["AskConsumesWhatItSaw"],
         violated("AskConsumesWhatItSaw")),
        ("fifoask.b", "ReaderIsolationFifoAsk_b", None, "INVARIANT", ["AskConsumesWhatItSaw"],
         violated("AskConsumesWhatItSaw")),
        ("fifoask.d.WorkSettlesWhenResolved", "ReaderIsolationFifoAsk_d", None, "PROPERTY",
         ["WorkSettlesWhenResolved"], "holds"),
        ("fifoask.d.WorkSettles", "ReaderIsolationFifoAsk_d", None, "PROPERTY", ["WorkSettles"],
         TEMPORAL_VIOLATED),
        ("fifoask.dnomark.WorkSettlesWhenResolved", "ReaderIsolationFifoAsk_dnomark", None, "PROPERTY",
         ["WorkSettlesWhenResolved"], TEMPORAL_VIOLATED),
        ("fifoask.dnomech.WorkSettlesWhenResolved", "ReaderIsolationFifoAsk_dnomech", None, "PROPERTY",
         ["WorkSettlesWhenResolved"], TEMPORAL_VIOLATED),
    ]
    + [(f"fifoask.d.{w}", "ReaderIsolationFifoAsk_d", None, "INVARIANT", [w], violated(w))
       for w in ["NeverDone", "NeverWaitsParked"]]
)


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
    kind = "INVARIANT" if prop == INVARIANT else "PROPERTY"
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


def check_reader_isolation(jar, out, numbers, labels=None):
    mismatches = []
    specs = out / "specs"
    specs.mkdir(exist_ok=True)
    for tla in HERE.glob("*.tla"):
        (specs / tla.name).write_text(tla.read_text())
    for label, cfg, case, kind, names, expected in RI_ROWS:
        if labels is not None and label not in labels:
            continue
        text = (HERE / f"{cfg}.cfg").read_text()
        if case:
            text += f'CONSTANT Case = "{case}"\n'
        text += f"{kind} {' '.join(names)}\n"
        (specs / f"ri-{label}.cfg").write_text(text)
        log = out / f"ReaderIsolation.{label}.log"
        cmd = [
            "nice", "java", "-XX:+UseParallelGC", "-cp", str(jar.resolve()), "tlc2.TLC",
            "-workers", "2", "-fp", "0", "-metadir", str((out / "states" / f"ri-{label}").resolve()),
            "-config", f"ri-{label}.cfg", f"{cfg.split('_')[0]}.tla",
        ]
        started = time.monotonic()
        with open(log, "w") as f:
            f.write(f"CMD: {' '.join(cmd)}\n")
            f.flush()
            code = subprocess.run(cmd, stdout=f, stderr=subprocess.STDOUT, cwd=specs).returncode
        seconds = time.monotonic() - started
        text = log.read_text()
        if code == TLC_HOLDS and "No error has been found" in text:
            result = "holds"
        elif code in (11, TLC_SAFETY, TLC_LIVENESS) and expected != "holds" and expected in text:
            result = "violated"
        elif code in (11, TLC_SAFETY, TLC_LIVENESS) and "Error:" in text:
            result = "violated other"
        else:
            raise ToolError(f"{log}: TLC exit {code} without a verdict")
        key = f"ReaderIsolation.{label}"
        numbers[f"{key}.result"] = result
        numbers[f"{key}.distinct_states"] = number(r"([\d,]+) distinct states found", text, log)
        numbers[f"{key}.seconds"] = f"{seconds:.1f}"
        print(f"{key}: {result}, {numbers[f'{key}.distinct_states']} distinct states, {seconds:.1f} s, "
              f"log {log}", flush=True)
        if result != ("holds" if expected == "holds" else "violated"):
            mismatches.append(f"{key}: {result}, expected {expected}")
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
        mismatches += check_reader_isolation(tla_jar(), out, numbers)
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
